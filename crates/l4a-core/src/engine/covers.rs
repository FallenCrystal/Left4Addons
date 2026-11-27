use super::L4aEngine;
use crate::db::repo::normalize_valid_remote_cover_url;
use crate::error::CoreError;
use crate::file_changes::same_file;
use crate::filter::evaluator::validate_filter_image_redirect_hop;
use l4a_logging::http;
use l4a_mirror::{MirrorError, RedirectRequestOptions, RedirectSendError};
use l4a_workshop::WorkshopError;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path};
use std::time::{Duration, SystemTime};

impl L4aEngine {
    pub fn read_cached_image(&self, cache_key: &str) -> Result<Vec<u8>, CoreError> {
        let key = Path::new(cache_key);
        let mut components = key.components();
        let invalid = || CoreError::InvalidInput("无效的缓存图片文件名".into());
        if cache_key.contains(['/', '\\', '\0'])
            || cache_key.contains("..")
            || (cfg!(windows) && cache_key.contains(':'))
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || !matches!(
                key.extension().and_then(|ext| ext.to_str()),
                Some("jpg" | "jpeg" | "png" | "webp")
            )
        {
            return Err(invalid());
        }

        let path = self.paths.image_cache_dir.join(key);
        let read_error = |err: io::Error| match err.kind() {
            io::ErrorKind::NotFound => CoreError::NotFound(format!("缓存图片不存在: {cache_key}")),
            _ => err.into(),
        };
        let metadata = fs::symlink_metadata(&path).map_err(read_error)?;
        if !metadata.is_file() {
            return Err(invalid());
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = options.open(path).map_err(read_error)?;
        if !same_file(&metadata, &file.metadata()?) {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub async fn fetch_workshop_cover(&self, url: &str) -> Result<(String, Vec<u8>), CoreError> {
        self.fetch_remote_cover(url, false).await
    }

    pub(crate) async fn fetch_filter_preview(
        &self,
        url: &str,
    ) -> Result<(String, Vec<u8>), CoreError> {
        self.fetch_remote_cover(url, true).await
    }

    async fn fetch_remote_cover(
        &self,
        url: &str,
        for_filter: bool,
    ) -> Result<(String, Vec<u8>), CoreError> {
        const MAX_COVER_BYTES: usize = 16 * 1024 * 1024;
        const COVER_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
        const CACHED_COVER_FORMATS: &[(&str, &str)] = &[
            ("jpg", "image/jpeg"),
            ("png", "image/png"),
            ("webp", "image/webp"),
            ("gif", "image/gif"),
        ];

        let url = normalize_valid_remote_cover_url(url)
            .ok_or_else(|| CoreError::InvalidInput("无效的封面图片地址".into()))?;
        let https_only = self.config.read().unwrap().https_only;
        self.mirror
            .validate_url_with_https_only(&url, https_only)
            .map_err(|err| WorkshopError::Network(err.to_string()))?;

        let cache_stem = if for_filter {
            // Only filter-validated fetches populate this cache. Scope entries to the immutable
            // mirror rules and HTTPS policy used for the whole redirect chain.
            let mut hash = Sha256::new();
            hash.update(self.mirror.rule_fingerprint());
            hash.update([u8::from(https_only)]);
            hash.update(url.as_bytes());
            format!("ws_filter_cover_v2_{}", hex::encode(&hash.finalize()[..16]))
        } else {
            format!(
                "ws_cover_{}",
                hex::encode(&Sha256::digest(url.as_bytes())[..16])
            )
        };
        let path_for_ext = |ext: &str| {
            self.paths
                .image_cache_dir
                .join(format!("{cache_stem}.{ext}"))
        };
        let mut stale_cached: Option<(&'static str, Vec<u8>)> = None;
        for &(ext, cached_mime) in CACHED_COVER_FORMATS {
            let cache_path = path_for_ext(ext);
            if let Ok(meta) = fs::symlink_metadata(&cache_path) {
                if meta.is_file() && meta.len() > 0 && meta.len() <= MAX_COVER_BYTES as u64 {
                    if let Ok(bytes) = fs::read(&cache_path) {
                        let is_fresh = meta
                            .modified()
                            .ok()
                            .and_then(|m| SystemTime::now().duration_since(m).ok())
                            .is_some_and(|age| age < COVER_CACHE_TTL);
                        if is_fresh {
                            return Ok((cached_mime.to_string(), bytes));
                        }
                        stale_cached = Some((cached_mime, bytes));
                        break;
                    }
                }
            }
        }

        let mut response = match self
            .mirror
            .send_with_redirects(
                reqwest::Method::GET,
                &url,
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(15)),
                    https_only_override: Some(https_only),
                    ..Default::default()
                },
                |req, _, _| req,
                |hop| {
                    if for_filter {
                        validate_filter_image_redirect_hop(hop)?;
                    }
                    Ok(())
                },
                |req, hop| http::send_redirect_hop_quiet(req, "workshop.cover", hop),
            )
            .await
        {
            Ok(redirected) => match redirected.response.error_for_status() {
                Ok(ok_resp) => ok_resp,
                Err(status_err) => {
                    if status_err
                        .status()
                        .is_some_and(|s| s.is_server_error() || s.as_u16() == 429)
                    {
                        if let Some((stale_mime, stale_bytes)) = stale_cached {
                            return Ok((stale_mime.to_string(), stale_bytes));
                        }
                    }
                    return Err(WorkshopError::Network(status_err.without_url().to_string()).into());
                }
            },
            Err(RedirectSendError::Rejected(err)) => return Err(err),
            Err(
                net_err @ (RedirectSendError::Request(_)
                | RedirectSendError::Timeout
                | RedirectSendError::Mirror(
                    MirrorError::TooManyRedirects | MirrorError::InvalidRedirect(_),
                )),
            ) => {
                if let Some((stale_mime, stale_bytes)) = stale_cached {
                    return Ok((stale_mime.to_string(), stale_bytes));
                }
                return Err(WorkshopError::Network(net_err.to_string()).into());
            }
            Err(RedirectSendError::Mirror(mirror_err)) => {
                return Err(WorkshopError::Network(mirror_err.to_string()).into());
            }
        };

        let header_mime = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|mime| mime.trim().to_ascii_lowercase())
            .filter(|mime| mime.starts_with("image/"));

        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() + chunk.len() > MAX_COVER_BYTES {
                        return Err(CoreError::InvalidInput("封面图片过大".into()));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(net_err) => {
                    if let Some((stale_mime, stale_bytes)) = stale_cached {
                        return Ok((stale_mime.to_string(), stale_bytes));
                    }
                    return Err(WorkshopError::Network(net_err.to_string()).into());
                }
            }
        }
        if bytes.is_empty() {
            if let Some((stale_mime, stale_bytes)) = stale_cached {
                return Ok((stale_mime.to_string(), stale_bytes));
            }
            return Err(CoreError::InvalidInput("封面图片为空".into()));
        }

        let detected = image::guess_format(&bytes).ok();
        if detected.is_none() {
            if let Some((stale_mime, stale_bytes)) = stale_cached {
                return Ok((stale_mime.to_string(), stale_bytes));
            }
        }
        let mime = detected
            .map(|format| format.to_mime_type().to_string())
            .or(header_mime)
            .ok_or_else(|| CoreError::InvalidInput("无法识别封面图片格式".into()))?;

        // 仅当字节头通过真实图片格式探测时才落盘缓存，避免网关 HTML 错误页伪造 Content-Type 污染磁盘缓存。
        let cache_ext = match detected {
            Some(image::ImageFormat::Jpeg) => Some("jpg"),
            Some(image::ImageFormat::Png) => Some("png"),
            Some(image::ImageFormat::WebP) => Some("webp"),
            Some(image::ImageFormat::Gif) => Some("gif"),
            _ => None,
        };
        if let Some(ext) = cache_ext {
            let dest = path_for_ext(ext);
            if let Ok(mut tmp) = tempfile::Builder::new()
                .prefix(".ws_cover_")
                .tempfile_in(&self.paths.image_cache_dir)
            {
                if tmp.write_all(&bytes).is_ok() && tmp.persist(&dest).is_ok() {
                    for &(other_ext, _) in CACHED_COVER_FORMATS {
                        if other_ext != ext {
                            let _ = fs::remove_file(path_for_ext(other_ext));
                        }
                    }
                }
            }
        }

        Ok((mime, bytes))
    }
}
