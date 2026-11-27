//! Shrinks oversized preview images before they are sent to a content filter backend.
//!
//! Shared by `l4a-core` and `l4a-server` through `#[path]` so that both sides apply the
//! same limits as `filter_runner.py` in this directory. The runner (and Pillow behind it)
//! rejects anything larger than `MAX_IMAGE_DIMENSION` / `MAX_IMAGE_PIXELS` as a potential
//! decompression bomb, which used to make legitimate Workshop items with very large
//! screenshots fail evaluation entirely.

use image::codecs::jpeg::JpegEncoder;
use image::{ImageReader, Limits};
use std::fmt;
use std::io::Cursor;
use tokio::sync::Semaphore;

/// Largest edge accepted by `filter_runner.py` (`MAX_IMAGE_DIMENSION`).
pub const FILTER_MAX_IMAGE_DIMENSION: u32 = 4_096;
/// Largest pixel count accepted by `filter_runner.py` (`MAX_IMAGE_PIXELS`).
pub const FILTER_MAX_IMAGE_PIXELS: u64 = 16_777_216;
/// Largest source image that will be decoded in order to shrink it (~134 MP).
pub const MAX_SHRINK_SOURCE_PIXELS: u64 = 1 << 27;
/// Largest source edge that will be decoded in order to shrink it.
pub const MAX_SHRINK_SOURCE_DIMENSION: u32 = 32_768;
/// MIME type of every image produced by [`shrink_for_filter`].
#[allow(dead_code)] // Used by `l4a-core`; `l4a-server` sniffs bytes instead.
pub const SHRUNK_IMAGE_MIME: &str = "image/jpeg";

/// Upper bound for decoder allocations (an RGBA8 image at the pixel cap needs ~512 MiB).
const MAX_SHRINK_DECODE_ALLOC: u64 = 768 * 1024 * 1024;
const SHRUNK_JPEG_QUALITY: u8 = 90;

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
    /// The image already fits the filter limits, or its header could not be read; the
    /// original bytes are passed through so the backend keeps its own validation.
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

fn read_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Synchronously downscales `bytes` when they exceed the filter limits.
///
/// Returns `Ok(None)` when no shrinking is needed or the header cannot be read.
pub fn shrink_for_filter(bytes: &[u8]) -> Result<Option<ShrunkImage>, ShrinkError> {
    let Some((width, height)) = read_dimensions(bytes) else {
        return Ok(None);
    };
    if fit_within_filter_limits(width, height).is_none() {
        return Ok(None);
    }
    if width > MAX_SHRINK_SOURCE_DIMENSION
        || height > MAX_SHRINK_SOURCE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_SHRINK_SOURCE_PIXELS
    {
        return Err(ShrinkError::SourceTooLarge { width, height });
    }

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| ShrinkError::Decode(err.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SHRINK_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SHRINK_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_SHRINK_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|err| ShrinkError::Decode(err.to_string()))?;

    // Use the decoded size in case the container header disagreed with the frame data.
    let source_size = (decoded.width(), decoded.height());
    let (out_w, out_h) =
        fit_within_filter_limits(source_size.0, source_size.1).unwrap_or(source_size);
    // Resize in the source colour type first so the full-size image is never duplicated,
    // then drop alpha exactly like Pillow's `convert("RGB")` in the runner.
    let resized = decoded.thumbnail_exact(out_w, out_h);
    drop(decoded);
    let rgb = resized.into_rgb8();

    let mut out = Vec::new();
    rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut out, SHRUNK_JPEG_QUALITY))
        .map_err(|err| ShrinkError::Encode(err.to_string()))?;
    Ok(Some(ShrunkImage {
        bytes: out,
        source_size,
        output_size: (rgb.width(), rgb.height()),
    }))
}

/// Async wrapper around [`shrink_for_filter`]: images that already fit are returned
/// without decoding, and oversized ones are shrunk on the blocking pool one at a time.
pub async fn prepare_for_filter(bytes: Vec<u8>) -> Result<PreparedImage, ShrinkError> {
    match read_dimensions(&bytes) {
        Some((w, h)) if fit_within_filter_limits(w, h).is_some() => {}
        _ => return Ok(PreparedImage::Original(bytes)),
    }
    let _permit = SHRINK_PERMITS
        .acquire()
        .await
        .map_err(|err| ShrinkError::Decode(err.to_string()))?;
    tokio::task::spawn_blocking(move || {
        Ok(match shrink_for_filter(&bytes)? {
            Some(shrunk) => PreparedImage::Shrunk(shrunk),
            None => PreparedImage::Original(bytes),
        })
    })
    .await
    .map_err(|err| ShrinkError::Decode(err.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

    fn encode(img: &image::DynamicImage, format: ImageFormat) -> Vec<u8> {
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

    #[test]
    fn shrink_passes_through_small_and_unreadable_images() {
        let small = encode(
            &RgbImage::from_pixel(64, 32, Rgb([1, 2, 3])).into(),
            ImageFormat::Png,
        );
        assert!(shrink_for_filter(&small).unwrap().is_none());
        assert!(shrink_for_filter(b"not an image").unwrap().is_none());
    }

    #[test]
    fn shrink_downscales_oversized_png_with_alpha_to_jpeg() {
        let big = encode(
            &RgbaImage::from_pixel(5000, 300, Rgba([200, 10, 10, 0])).into(),
            ImageFormat::Png,
        );
        let shrunk = shrink_for_filter(&big).unwrap().expect("must shrink");
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

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn shrink_rejects_sources_beyond_the_decode_budget() {
        // Hand-built PNG header claiming 20000x20000 (400 MP); never decoded.
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend_from_slice(&20_000u32.to_be_bytes());
        ihdr.extend_from_slice(&20_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d".to_vec();
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        // Header parsing stops at the first IDAT chunk, so an empty one is enough.
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&crc32(b"IDAT").to_be_bytes());
        let err = shrink_for_filter(&png).unwrap_err();
        assert!(matches!(
            err,
            ShrinkError::SourceTooLarge {
                width: 20_000,
                height: 20_000
            }
        ));
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
