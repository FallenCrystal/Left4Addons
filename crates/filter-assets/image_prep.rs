//! Shrinks oversized preview images before they are sent to a content filter backend.
//!
//! Shared by `l4a-core` and `l4a-server` through `#[path]` so that both sides apply the
//! same limits as `filter_runner.py` in this directory. The runner (and Pillow behind it)
//! rejects anything larger than `MAX_IMAGE_DIMENSION` / `MAX_IMAGE_PIXELS` as a potential
//! decompression bomb, which used to make legitimate Workshop items with very large
//! screenshots fail evaluation entirely.

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbImage};
use std::fmt;
use std::io::{self, Cursor, Write};
use tokio::sync::Semaphore;

/// Largest edge accepted by `filter_runner.py` (`MAX_IMAGE_DIMENSION`).
pub const FILTER_MAX_IMAGE_DIMENSION: u32 = 4_096;
/// Largest pixel count accepted by `filter_runner.py` (`MAX_IMAGE_PIXELS`).
pub const FILTER_MAX_IMAGE_PIXELS: u64 = 16_777_216;
/// Largest encoded image accepted by `filter_runner.py` (`MAX_SINGLE_IMAGE_BYTES`).
pub const FILTER_MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Largest source image that will be decoded in order to shrink it (~134 MP).
const MAX_SHRINK_SOURCE_PIXELS: u64 = 1 << 27;
/// Largest source edge that will be decoded in order to shrink it.
const MAX_SHRINK_SOURCE_DIMENSION: u32 = 32_768;
/// MIME type of every image produced by [`shrink_for_filter`].
#[allow(dead_code)] // Used by `l4a-core`; `l4a-server` sniffs bytes instead.
pub const SHRUNK_IMAGE_MIME: &str = "image/jpeg";

/// Budget for the estimated peak working set of a resize job.
const MAX_SHRINK_WORKING_SET: u64 = 768 * 1024 * 1024;
/// Allowance for codec bookkeeping and temporary row buffers.
const DECODER_HEADROOM: u64 = 32 * 1024 * 1024;
/// The PNG decoder receives its own limit for metadata and internal buffers at construction.
const PNG_DECODER_ALLOC: u64 = 64 * 1024 * 1024;
const SHRUNK_JPEG_QUALITIES: [u8; 3] = [90, 80, 70];

/// Decoding a huge image needs hundreds of MiB, so only one runs at a time per process.
static SHRINK_PERMITS: Semaphore = Semaphore::const_new(1);

#[derive(Debug)]
pub struct ShrunkImage {
    /// JPEG-encoded bytes ([`SHRUNK_IMAGE_MIME`]).
    pub bytes: Vec<u8>,
    pub source_size: (u32, u32),
    pub output_size: (u32, u32),
}

#[derive(Debug)]
pub enum PreparedImage {
    /// The dimensions fit the filter limits or could not be read from the header.
    /// The backend validates these bytes.
    Original(Vec<u8>),
    Shrunk(ShrunkImage),
}

#[derive(Debug)]
pub enum ShrinkError {
    SourceTooLarge { width: u32, height: u32 },
    Decode(String),
    Encode(String),
}

impl fmt::Display for ShrinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceTooLarge { width, height } => write!(
                f,
                "预览图分辨率过大 ({width}x{height})，超出可安全缩放的上限"
            ),
            Self::Decode(err) => write!(f, "解码超大预览图失败: {err}"),
            Self::Encode(err) => write!(f, "重新编码缩放后的预览图失败: {err}"),
        }
    }
}

impl std::error::Error for ShrinkError {}

/// Returns the largest size with the same aspect ratio that fits the filter limits, or
/// `None` when `width x height` already fits (or is degenerate).
pub fn fit_within_filter_limits(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let pixels = u64::from(width) * u64::from(height);
    if width <= FILTER_MAX_IMAGE_DIMENSION
        && height <= FILTER_MAX_IMAGE_DIMENSION
        && pixels <= FILTER_MAX_IMAGE_PIXELS
    {
        return None;
    }

    let (w, h) = (f64::from(width), f64::from(height));
    let max_edge = f64::from(FILTER_MAX_IMAGE_DIMENSION);
    let scale = (max_edge / w)
        .min(max_edge / h)
        .min((FILTER_MAX_IMAGE_PIXELS as f64 / (w * h)).sqrt());
    let mut out_w = ((w * scale).floor() as u32).clamp(1, FILTER_MAX_IMAGE_DIMENSION);
    let mut out_h = ((h * scale).floor() as u32).clamp(1, FILTER_MAX_IMAGE_DIMENSION);
    // Guard against floating-point rounding pushing the result over the pixel cap.
    while u64::from(out_w) * u64::from(out_h) > FILTER_MAX_IMAGE_PIXELS {
        if out_w >= out_h {
            out_w -= 1;
        } else {
            out_h -= 1;
        }
    }
    Some((out_w, out_h))
}

/// Reads the pixel size from a PNG, GIF, JPEG or WebP header without decoding the image.
pub fn sniff_image_dimensions(bytes: &[u8]) -> Result<(u32, u32, ImageFormat), &'static str> {
    // 1. PNG: 8-byte signature `\x89PNG\r\n\x1a\n` followed by `IHDR` chunk (width & height big-endian u32).
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err("损坏的 PNG 图片头 (缺少 IHDR)");
        }
        let ihdr_len = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if ihdr_len != 13 {
            return Err("损坏的 PNG IHDR 块长度");
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Ok((w, h, ImageFormat::Png));
    }

    // 2. GIF: `GIF87a` or `GIF89a` followed by logical screen width & height (little-endian u16).
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if bytes.len() < 10 {
            return Err("损坏的 GIF 图片头");
        }
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Ok((w, h, ImageFormat::Gif));
    }

    // 3. JPEG: `\xFF\xD8` followed by segments until SOF0..SOF3 / SOF5..SOF7 / SOF9..SOF11 / SOF13..SOF15.
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut pos = 2usize;
        while pos < bytes.len() {
            if bytes[pos] != 0xFF {
                return Err("损坏的 JPEG 段标记结构");
            }
            while pos < bytes.len() && bytes[pos] == 0xFF {
                pos += 1;
            }
            if pos >= bytes.len() {
                break;
            }
            let marker = bytes[pos];
            pos += 1;

            match marker {
                0x00 => {
                    return Err("损坏的 JPEG 头部填充字节");
                }
                // Standalone markers without length payload
                0x01 | 0xD0..=0xD8 => continue,
                // EOI or SOS reached before any SOF frame header
                0xD9 | 0xDA => break,
                _ => {}
            }

            if pos + 2 > bytes.len() {
                return Err("截断的 JPEG 段长度头");
            }
            let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
            if seg_len < 2 || pos + seg_len > bytes.len() {
                return Err("JPEG 段长度越界或损坏");
            }

            if matches!(
                marker,
                0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
            ) {
                if seg_len < 8 {
                    return Err("损坏的 JPEG SOF 帧头");
                }
                let h = u16::from_be_bytes([bytes[pos + 3], bytes[pos + 4]]) as u32;
                let w = u16::from_be_bytes([bytes[pos + 5], bytes[pos + 6]]) as u32;
                return Ok((w, h, ImageFormat::Jpeg));
            }

            pos += seg_len;
        }
        return Err("JPEG 图片缺少有效的 SOF 尺寸头");
    }

    // 4. WebP: `RIFF....WEBP` followed by `VP8 `, `VP8L`, or `VP8X` chunk.
    if bytes.len() >= 20 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk_type = &bytes[12..16];
        let chunk_size = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
        match chunk_type {
            b"VP8 " => {
                if chunk_size < 10 || bytes.len() < 30 {
                    return Err("截断的 WebP VP8 帧头");
                }
                if (bytes[20] & 0x01) != 0 || bytes[23..26] != [0x9D, 0x01, 0x2A] {
                    return Err("损坏的 WebP VP8 关键帧签名");
                }
                let w = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
                let h = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
                return Ok((w, h, ImageFormat::WebP));
            }
            b"VP8L" => {
                if chunk_size < 5 || bytes.len() < 25 || bytes[20] != 0x2F {
                    return Err("损坏的 WebP VP8L 无损帧头");
                }
                let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
                let w = (bits & 0x3FFF) + 1;
                let h = ((bits >> 14) & 0x3FFF) + 1;
                return Ok((w, h, ImageFormat::WebP));
            }
            b"VP8X" => {
                if chunk_size < 10 || bytes.len() < 30 {
                    return Err("截断的 WebP VP8X 扩展头");
                }
                let w =
                    (bytes[24] as u32 | ((bytes[25] as u32) << 8) | ((bytes[26] as u32) << 16)) + 1;
                let h =
                    (bytes[27] as u32 | ((bytes[28] as u32) << 8) | ((bytes[29] as u32) << 16)) + 1;
                return Ok((w, h, ImageFormat::WebP));
            }
            _ => {
                return Err("不支持或损坏的 WebP 块类型");
            }
        }
    }

    Err("不支持的图片格式（仅支持 PNG、JPEG、GIF、WebP）")
}

fn decoder_scratch_bytes(format: ImageFormat, width: u32, height: u32, input_len: u64) -> u64 {
    let (width, height) = (u64::from(width), u64::from(height));
    match format {
        // Zune can keep up to four full i16 coefficient planes for progressive or
        // multi-scan sequential JPEGs. Each dimension can be padded by up to 31 pixels
        // (4 x 8-pixel blocks per MCU). The image adapter also copies the compressed input.
        ImageFormat::Jpeg => 8 * (width + 31) * (height + 31) + input_len,
        ImageFormat::Png => PNG_DECODER_ALLOC,
        // An animated WebP can hold an RGBA frame and an RGBA canvas alongside the output.
        // Allow for both buffers and padding in the decoder's lossy planes.
        ImageFormat::WebP => 8 * (width + 15) * (height + 15),
        // The GIF decoder accounts for its intermediate frame with Limits::max_alloc.
        _ => 0,
    }
}

fn shrink_for_filter(
    bytes: &[u8],
    format: ImageFormat,
    source_size: (u32, u32),
    target_size: (u32, u32),
) -> Result<ShrunkImage, ShrinkError> {
    let (width, height) = source_size;
    let (out_w, out_h) = target_size;
    let source_too_large = || ShrinkError::SourceTooLarge { width, height };
    let decode_error = |err: image::ImageError| ShrinkError::Decode(err.to_string());

    let input_len = bytes.len() as u64;
    let available = MAX_SHRINK_WORKING_SET.saturating_sub(input_len);
    let scratch = decoder_scratch_bytes(format, width, height, input_len);
    let decode_budget = available
        .checked_sub(DECODER_HEADROOM + scratch)
        .ok_or_else(source_too_large)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(decode_budget);

    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut init_limits = limits.clone();
    if format == ImageFormat::Png {
        init_limits.max_alloc = Some(PNG_DECODER_ALLOC);
    }
    reader.limits(init_limits);
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    if decoder.dimensions() != source_size {
        return Err(ShrinkError::Decode("图片头与解码器报告的尺寸不一致".into()));
    }

    // Reserve the decoded pixels before allocating them, leaving the remainder for
    // decoders (such as GIF) that honor max_alloc for additional buffers.
    let decoded_bytes = decoder.total_bytes();
    limits
        .reserve(decoded_bytes)
        .map_err(|_| source_too_large())?;
    let thumbnail_bytes =
        u64::from(out_w) * u64::from(out_h) * u64::from(decoder.color_type().bytes_per_pixel());
    if decoded_bytes + thumbnail_bytes > available {
        return Err(source_too_large());
    }
    decoder.set_limits(limits).map_err(decode_error)?;
    let decoded = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    // Resize in the source colour type first so the full-size image is never duplicated,
    // then drop alpha exactly like Pillow's convert("RGB") in the runner.
    let resized = decoded.thumbnail_exact(out_w, out_h);
    drop(decoded);

    let (bytes, output_size) = encode_filter_jpeg(resized.into_rgb8(), FILTER_MAX_IMAGE_BYTES)?;
    Ok(ShrunkImage {
        bytes,
        source_size,
        output_size,
    })
}

struct JpegOutput {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl Write for JpegOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.max_bytes - self.bytes.len() {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.bytes.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_filter_jpeg(
    mut rgb: RgbImage,
    max_bytes: usize,
) -> Result<(Vec<u8>, (u32, u32)), ShrinkError> {
    let mut output = JpegOutput {
        bytes: Vec::new(),
        max_bytes,
    };
    loop {
        for quality in SHRUNK_JPEG_QUALITIES {
            output.bytes.clear();
            let encoder = JpegEncoder::new_with_quality(&mut output, quality);
            match rgb.write_with_encoder(encoder) {
                Ok(()) => return Ok((output.bytes, rgb.dimensions())),
                Err(image::ImageError::IoError(err))
                    if err.kind() == io::ErrorKind::FileTooLarge =>
                {
                    continue;
                }
                Err(err) => return Err(ShrinkError::Encode(err.to_string())),
            }
        }

        let (width, height) = rgb.dimensions();
        if (width, height) == (1, 1) {
            return Err(ShrinkError::Encode("无法在体积上限内编码 JPEG".into()));
        }
        rgb = image::imageops::thumbnail(&rgb, (width / 2).max(1), (height / 2).max(1));
    }
}

/// Probe dimensions without a decoder. Oversized images are shrunk on the blocking pool
/// one at a time, with their source size checked before decoder initialization.
pub async fn prepare_for_filter(bytes: Vec<u8>) -> Result<PreparedImage, ShrinkError> {
    let Ok((width, height, format)) = sniff_image_dimensions(&bytes) else {
        return Ok(PreparedImage::Original(bytes));
    };
    let Some(target_size) = fit_within_filter_limits(width, height) else {
        return Ok(PreparedImage::Original(bytes));
    };
    if width > MAX_SHRINK_SOURCE_DIMENSION
        || height > MAX_SHRINK_SOURCE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_SHRINK_SOURCE_PIXELS
    {
        return Err(ShrinkError::SourceTooLarge { width, height });
    }

    run_resize(&SHRINK_PERMITS, move || {
        shrink_for_filter(&bytes, format, (width, height), target_size).map(PreparedImage::Shrunk)
    })
    .await
}

async fn run_resize(
    permits: &'static Semaphore,
    resize: impl FnOnce() -> Result<PreparedImage, ShrinkError> + Send + 'static,
) -> Result<PreparedImage, ShrinkError> {
    let permit = permits
        .acquire()
        .await
        .map_err(|err| ShrinkError::Decode(err.to_string()))?;
    tokio::task::spawn_blocking(move || {
        // The blocking job can outlive a canceled request.
        let _permit = permit;
        resize()
    })
    .await
    .map_err(|err| ShrinkError::Decode(err.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, Rgba, RgbaImage};

    fn encode(img: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn fit_keeps_images_within_limits_untouched() {
        assert_eq!(fit_within_filter_limits(1920, 1080), None);
        assert_eq!(fit_within_filter_limits(4096, 4096), None);
        assert_eq!(fit_within_filter_limits(0, 100), None);
    }

    #[test]
    fn fit_scales_oversized_images_preserving_aspect_ratio() {
        // The 72,318,016-pixel image from the original bug report.
        let (w, h) = fit_within_filter_limits(8496, 8512).unwrap();
        assert!(w <= FILTER_MAX_IMAGE_DIMENSION && h <= FILTER_MAX_IMAGE_DIMENSION);
        assert!(u64::from(w) * u64::from(h) <= FILTER_MAX_IMAGE_PIXELS);
        assert!(
            w >= 4080 && h >= 4090,
            "should not shrink more than needed: {w}x{h}"
        );

        assert_eq!(fit_within_filter_limits(8192, 100), Some((4096, 50)));
        assert_eq!(fit_within_filter_limits(100_000, 1), Some((4096, 1)));
        let (w, h) = fit_within_filter_limits(4097, 4097).unwrap();
        assert!(w <= 4096 && h <= 4096);
    }

    #[tokio::test]
    async fn shrink_passes_through_small_and_unreadable_images() {
        let small = encode(
            &RgbImage::from_pixel(64, 32, Rgb([1, 2, 3])).into(),
            ImageFormat::Png,
        );
        for original in [small, b"not an image".to_vec()] {
            let PreparedImage::Original(bytes) =
                prepare_for_filter(original.clone()).await.unwrap()
            else {
                panic!("expected original bytes");
            };
            assert_eq!(bytes, original);
        }
    }

    #[tokio::test]
    async fn shrink_downscales_oversized_png_with_alpha_to_jpeg() {
        let big = encode(
            &RgbaImage::from_pixel(5000, 300, Rgba([200, 10, 10, 0])).into(),
            ImageFormat::Png,
        );
        let PreparedImage::Shrunk(shrunk) = prepare_for_filter(big).await.unwrap() else {
            panic!("oversized PNG must be shrunk");
        };
        assert_eq!(shrunk.source_size, (5000, 300));
        assert_eq!(shrunk.output_size, (4096, 245));
        assert_eq!(
            image::guess_format(&shrunk.bytes).unwrap(),
            ImageFormat::Jpeg
        );
        let reread = image::load_from_memory(&shrunk.bytes).unwrap();
        assert_eq!((reread.width(), reread.height()), (4096, 245));
        // Alpha is dropped rather than composited, matching Pillow's convert("RGB").
        let px = reread.to_rgb8().get_pixel(10, 10).0;
        assert!(
            px[0] > 150 && px[1] < 60 && px[2] < 60,
            "unexpected colour {px:?}"
        );
    }

    #[tokio::test]
    async fn shrink_checks_png_dimensions_before_initializing_the_decoder() {
        // Only the dimensions are present. A decoder would fail on the truncated IHDR.
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&20_000u32.to_be_bytes());
        let err = prepare_for_filter(png).await.unwrap_err();
        assert!(matches!(
            err,
            ShrinkError::SourceTooLarge {
                width: 20_000,
                height: 20_000
            }
        ));
    }

    #[tokio::test]
    async fn shrink_rejects_jpeg_coefficient_buffers_beyond_the_budget() {
        // These dimensions fit the source pixel cap. The SOF is enough to reject
        // the estimated JPEG working set before decoder initialization.
        let size = 11_500u16;
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xc2, 0, 17, 8];
        jpeg.extend_from_slice(&size.to_be_bytes());
        jpeg.extend_from_slice(&size.to_be_bytes());
        jpeg.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);
        assert!(matches!(
            prepare_for_filter(jpeg).await.unwrap_err(),
            ShrinkError::SourceTooLarge {
                width: 11_500,
                height: 11_500
            }
        ));
    }

    #[test]
    fn jpeg_encoding_reduces_quality_then_dimensions_to_fit_the_byte_limit() {
        let rgb = RgbImage::from_fn(128, 96, |x, y| {
            Rgb([
                (19 * x + 13 * y) as u8,
                (5 * x + 37 * y) as u8,
                (11 * (x ^ y)) as u8,
            ])
        });
        let mut initial = Vec::new();
        rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut initial, 90))
            .unwrap();

        let quality_budget = initial.len() - 1;
        let (bytes, size) = encode_filter_jpeg(rgb.clone(), quality_budget).unwrap();
        assert!(bytes.len() <= quality_budget);
        assert_eq!(size, rgb.dimensions());

        let smaller_budget = 1024;
        let (bytes, size) = encode_filter_jpeg(rgb, smaller_budget).unwrap();
        assert!(bytes.len() <= smaller_budget);
        assert!(size.0 < 128 && size.1 < 96);
        assert_eq!(size.0 * 96, size.1 * 128);
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .into_rgb8()
                .dimensions(),
            size
        );
    }

    #[tokio::test]
    async fn canceled_resize_holds_its_permit_until_the_worker_exits() {
        static PERMITS: Semaphore = Semaphore::const_new(1);
        let (started, wait_for_start) = tokio::sync::oneshot::channel();
        let (finish, wait_for_finish) = std::sync::mpsc::channel();
        let request = tokio::spawn(run_resize(&PERMITS, move || {
            started.send(()).unwrap();
            let _ = wait_for_finish.recv();
            Ok(PreparedImage::Original(Vec::new()))
        }));
        wait_for_start.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(PERMITS.try_acquire().is_err());

        finish.send(()).unwrap();
        let _permit = tokio::time::timeout(std::time::Duration::from_secs(5), PERMITS.acquire())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn prepare_returns_original_bytes_when_no_shrink_needed() {
        let small = encode(
            &RgbImage::from_pixel(16, 16, Rgb([9, 9, 9])).into(),
            ImageFormat::Jpeg,
        );
        match prepare_for_filter(small.clone()).await.unwrap() {
            PreparedImage::Original(bytes) => assert_eq!(bytes, small),
            PreparedImage::Shrunk(_) => panic!("small image must not be re-encoded"),
        }

        let big = encode(
            &RgbImage::from_pixel(4100, 64, Rgb([9, 9, 9])).into(),
            ImageFormat::Jpeg,
        );
        match prepare_for_filter(big).await.unwrap() {
            PreparedImage::Shrunk(s) => assert_eq!(s.output_size, (4096, 63)),
            PreparedImage::Original(_) => panic!("oversized image must be shrunk"),
        }
    }
}
