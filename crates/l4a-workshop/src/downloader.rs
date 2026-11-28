use crate::error::WorkshopError;
use futures_util::StreamExt;
use l4a_logging::http;
use l4a_mirror::{MirrorRouter, RedirectRequestOptions, RedirectedResponse, ResolvedRequest};
use l4a_protocol::WorkshopId;
use reqwest::header::{CONTENT_RANGE, ETAG, RANGE};
use reqwest::{Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(200);
const SPEED_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadProgress {
    pub workshop_id: WorkshopId,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: u8,
    pub speed_bps: Option<u64>,
}

pub type ProgressSink = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub workshop_id: WorkshopId,
    pub url: String,
    pub dest_path: PathBuf,
    pub max_retries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadResult {
    pub dest_path: PathBuf,
    pub bytes_written: u64,
    pub resumed_from: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeMetadata {
    workshop_id: WorkshopId,
    resolved_url: String,
    resource_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signed_resource_fingerprint: Option<String>,
    dest_file_name: String,
    total_size: Option<u64>,
    etag: Option<String>,
}

impl ResumeMetadata {
    fn matches_resource(&self, fingerprint: &str, signed_fingerprint: Option<&str>) -> bool {
        self.resource_fingerprint.as_deref() == Some(fingerprint)
            || (signed_fingerprint.is_some()
                && self.signed_resource_fingerprint.as_deref() == signed_fingerprint)
    }

    fn strong_etag(&self) -> Option<&str> {
        self.etag.as_deref().filter(|etag| is_strong_etag(etag))
    }
}

pub struct ResilientDownloader {
    cache_dir: PathBuf,
    mirror: Arc<MirrorRouter>,
    connect_timeout: Duration,
    read_timeout: Duration,
    progress_emit_interval: Duration,
    speed_refresh_interval: Duration,
}

impl ResilientDownloader {
    pub fn new(cache_dir: impl AsRef<Path>, mirror: Arc<MirrorRouter>) -> Self {
        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
            mirror,
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(60),
            progress_emit_interval: PROGRESS_EMIT_INTERVAL,
            speed_refresh_interval: SPEED_REFRESH_INTERVAL,
        }
    }

    pub fn with_progress_intervals(
        mut self,
        progress_emit_interval: Duration,
        speed_refresh_interval: Duration,
    ) -> Self {
        self.progress_emit_interval = progress_emit_interval;
        self.speed_refresh_interval = speed_refresh_interval.max(Duration::from_millis(1));
        self
    }

    pub async fn download(
        &self,
        req: &DownloadRequest,
        cancel: CancellationToken,
        on_progress: Option<ProgressSink>,
    ) -> Result<DownloadResult, WorkshopError> {
        fs::create_dir_all(&self.cache_dir)?;
        if let Some(parent) = req.dest_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let started = std::time::Instant::now();
        let url = l4a_logging::redact::redact_url_str(&req.url);
        tracing::info!(
            workshop_id = %req.workshop_id,
            url = %url,
            dest = %req.dest_path.display(),
            max_retries = req.max_retries,
            "Starting Workshop file download"
        );
        let mut last_err: Option<WorkshopError> = None;
        for attempt in 0..=req.max_retries {
            if cancel.is_cancelled() {
                self.cleanup_partial(req.workshop_id);
                tracing::info!(workshop_id = %req.workshop_id, "Download cancelled");
                return Err(WorkshopError::Cancelled);
            }

            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(WorkshopError::Cancelled),
                result = self.download_once(req, cancel.clone(), on_progress.as_ref()) => result,
            };
            match result {
                Ok(result) => {
                    let elapsed = started.elapsed();
                    let transferred = result.bytes_written.saturating_sub(result.resumed_from);
                    tracing::info!(
                        workshop_id = %req.workshop_id,
                        bytes = result.bytes_written,
                        resumed_from = result.resumed_from,
                        attempts = attempt + 1,
                        elapsed_ms = elapsed.as_millis() as u64,
                        avg_kib_s = (transferred as f64 / 1024.0 / elapsed.as_secs_f64().max(0.001)) as u64,
                        "Workshop file download completed"
                    );
                    return Ok(result);
                }
                Err(WorkshopError::Cancelled) => {
                    self.cleanup_partial(req.workshop_id);
                    tracing::info!(workshop_id = %req.workshop_id, "Download cancelled");
                    return Err(WorkshopError::Cancelled);
                }
                Err(err) => {
                    let retryable = is_retryable(&err);
                    tracing::warn!(
                        workshop_id = %req.workshop_id,
                        attempt = attempt + 1,
                        retryable,
                        error = %err,
                        "Download attempt failed"
                    );
                    last_err = Some(err);
                    if !retryable || attempt >= req.max_retries {
                        break;
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            self.cleanup_partial(req.workshop_id);
                            return Err(WorkshopError::Cancelled);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200 * (attempt as u64 + 1))) => {}
                    }
                }
            }
        }

        let err = last_err.unwrap_or_else(|| WorkshopError::Network("未知下载错误".into()));
        tracing::warn!(
            workshop_id = %req.workshop_id,
            elapsed_ms = started.elapsed().as_millis() as u64,
            error = %err,
            "Workshop file download failed"
        );
        Err(err)
    }

    async fn download_once(
        &self,
        req: &DownloadRequest,
        cancel: CancellationToken,
        on_progress: Option<&ProgressSink>,
    ) -> Result<DownloadResult, WorkshopError> {
        let partial_path = self.partial_path(req.workshop_id);
        let meta_path = self.meta_path(req.workshop_id);
        let dest_name = req
            .dest_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("addon.vpk")
            .to_string();

        let resolved = self
            .mirror
            .validate_url(&req.url)
            .map_err(|e| WorkshopError::Network(e.to_string()))?;

        let stored_meta = fs::read(&meta_path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<ResumeMetadata>(&raw).ok())
            .filter(|meta| {
                meta.workshop_id == req.workshop_id
                    && meta.dest_file_name == dest_name
                    && meta.resource_fingerprint.is_some()
                    && meta.strong_etag().is_some()
                    && same_download_entry(&meta.resolved_url, &resolved.url)
            });
        let mut resume = stored_meta.and_then(|meta| {
            let offset = fs::metadata(&partial_path).ok()?.len();
            (offset > 0).then_some((offset, meta))
        });
        let mut purpose = if resume.is_some() {
            "workshop.download.resume"
        } else {
            "workshop.download"
        };
        let (redirected, fingerprints, lengths, etag) = loop {
            let offset = resume.as_ref().map(|(offset, _)| *offset);
            let redirected = self.send_request(&req.url, offset, purpose).await?;
            let response = &redirected.response;
            let status = response.status();
            if status == StatusCode::RANGE_NOT_SATISFIABLE && resume.is_some() {
                resume = None;
                purpose = "workshop.download.restart";
                continue;
            }
            if !matches!(status, StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
                let body = redirected.response.text().await.unwrap_or_default();
                return Err(WorkshopError::HttpStatus {
                    status: status.as_u16(),
                    body,
                });
            }
            if status == StatusCode::OK {
                resume = None;
            }
            let fingerprints = resource_fingerprints(&redirected.final_resolved);
            let previous_total = resume
                .as_ref()
                .and_then(|(_, previous)| previous.total_size);
            let lengths = response_lengths(response, offset.unwrap_or(0), previous_total);
            let etag = response_strong_etag(response);
            if let Some((_, previous)) = &resume {
                if !previous.matches_resource(&fingerprints.0, fingerprints.1.as_deref())
                    || previous.strong_etag() != etag
                    || lengths.is_err()
                {
                    // Preserve the partial until a usable replacement response arrives.
                    resume = None;
                    purpose = "workshop.download.restart";
                    continue;
                }
            }
            let etag = etag.map(str::to_owned);
            break (redirected, fingerprints, lengths?, etag);
        };
        let resume_from = resume.map_or(0, |(offset, _)| offset);
        let (fingerprint, signed_fingerprint) = fingerprints;
        let (total_size, range_end) = lengths;
        let response = redirected.response;

        let meta = ResumeMetadata {
            workshop_id: req.workshop_id,
            resolved_url: redirected.initial_resolved.url,
            resource_fingerprint: Some(fingerprint),
            signed_resource_fingerprint: signed_fingerprint,
            dest_file_name: dest_name,
            total_size,
            etag,
        };
        let mut file = if resume_from > 0 {
            let mut file = OpenOptions::new().write(true).open(&partial_path)?;
            if file.seek(SeekFrom::End(0))? != resume_from {
                return Err(WorkshopError::IntegrityMismatch(
                    "本地断点文件长度已变化".into(),
                ));
            }
            file
        } else {
            fs::File::create(&partial_path)?
        };
        fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&meta).unwrap_or_default(),
        )?;

        let mut downloaded = resume_from;
        let mut stream = response.bytes_stream();
        let start = tokio::time::Instant::now();
        let mut last_speed_at = start;
        let mut last_speed_bytes = downloaded;
        let mut current_speed_bps = 0u64;
        let mut last_emit_at = start;
        let mut speed_tick = tokio::time::interval_at(
            start + self.speed_refresh_interval,
            self.speed_refresh_interval,
        );
        speed_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        if let Some(sink) = on_progress {
            sink(DownloadProgress {
                workshop_id: req.workshop_id,
                downloaded,
                total: total_size,
                percent: calc_percent(downloaded, total_size),
                speed_bps: Some(0),
            });
        }

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    drop(file);
                    return Err(WorkshopError::Cancelled);
                }
                _ = speed_tick.tick() => {
                    let now = tokio::time::Instant::now();
                    let elapsed = now.duration_since(last_speed_at).as_secs_f64();
                    let delta = downloaded.saturating_sub(last_speed_bytes);
                    current_speed_bps = if elapsed > 0.0 {
                        (delta as f64 / elapsed).round() as u64
                    } else {
                        0
                    };
                    last_speed_at = now;
                    last_speed_bytes = downloaded;
                    last_emit_at = now;
                    if let Some(sink) = on_progress {
                        sink(DownloadProgress {
                            workshop_id: req.workshop_id,
                            downloaded,
                            total: total_size,
                            percent: calc_percent(downloaded, total_size),
                            speed_bps: Some(current_speed_bps),
                        });
                    }
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            if range_end.is_some_and(|end| bytes.len() as u64 > end - downloaded) {
                                file.set_len(resume_from)?;
                                return Err(WorkshopError::IntegrityMismatch(
                                    "206 响应体超出 Content-Range 声明的范围".into(),
                                ));
                            }
                            file.write_all(&bytes)?;
                            downloaded += bytes.len() as u64;
                            let now = tokio::time::Instant::now();
                            if now.duration_since(last_emit_at) >= self.progress_emit_interval {
                                last_emit_at = now;
                                if let Some(sink) = on_progress {
                                    sink(DownloadProgress {
                                        workshop_id: req.workshop_id,
                                        downloaded,
                                        total: total_size,
                                        percent: calc_percent(downloaded, total_size),
                                        speed_bps: Some(current_speed_bps),
                                    });
                                }
                            }
                        }
                        Some(Err(err)) => {
                            return Err(WorkshopError::Network(err.to_string()));
                        }
                        None => break,
                    }
                }
            }
        }

        file.flush()?;
        drop(file);

        if let Some(expected) = total_size {
            if downloaded != expected {
                return Err(WorkshopError::Network(format!(
                    "响应体流提前结束或被截断: 预期 {expected} 字节, 实际 {downloaded} 字节"
                )));
            }
        }

        if fs::rename(&partial_path, &req.dest_path).is_err() {
            let mut tmp_os = req.dest_path.as_os_str().to_os_string();
            tmp_os.push(".l4a_tmp");
            let tmp_path = PathBuf::from(tmp_os);
            if let Err(err) = fs::copy(&partial_path, &tmp_path)
                .and_then(|_| fs::rename(&tmp_path, &req.dest_path))
            {
                let _ = fs::remove_file(&tmp_path);
                return Err(WorkshopError::Io(err));
            }
            let _ = fs::remove_file(&partial_path);
        }
        let _ = fs::remove_file(&meta_path);

        if let Some(sink) = on_progress {
            sink(DownloadProgress {
                workshop_id: req.workshop_id,
                downloaded,
                total: total_size.or(Some(downloaded)),
                percent: 100,
                speed_bps: Some(current_speed_bps),
            });
        }

        Ok(DownloadResult {
            dest_path: req.dest_path.clone(),
            bytes_written: downloaded,
            resumed_from: resume_from,
        })
    }

    async fn send_request(
        &self,
        url: &str,
        offset: Option<u64>,
        purpose: &'static str,
    ) -> Result<RedirectedResponse, WorkshopError> {
        let range = offset.map(|offset| format!("bytes={offset}-"));
        self.mirror
            .send_with_redirects(
                Method::GET,
                url,
                RedirectRequestOptions {
                    connect_timeout: Some(self.connect_timeout),
                    read_timeout: Some(self.read_timeout),
                    ..Default::default()
                },
                |builder, _, _| match &range {
                    Some(range) => builder.header(RANGE, range),
                    None => builder,
                },
                |_| Ok::<(), WorkshopError>(()),
                |builder, hop| http::send_redirect_hop(builder, purpose, hop),
            )
            .await
            .map_err(|e| WorkshopError::Network(e.to_string()))
    }

    fn partial_path(&self, id: WorkshopId) -> PathBuf {
        self.cache_dir.join(format!("{id}.vpk.part"))
    }

    fn meta_path(&self, id: WorkshopId) -> PathBuf {
        self.cache_dir.join(format!("{id}.part.meta.json"))
    }

    fn cleanup_partial(&self, id: WorkshopId) {
        let _ = fs::remove_file(self.partial_path(id));
        let _ = fs::remove_file(self.meta_path(id));
    }
}

fn response_lengths(
    response: &Response,
    offset: u64,
    previous_total: Option<u64>,
) -> Result<(Option<u64>, Option<u64>), WorkshopError> {
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Ok((response.content_length(), None));
    }
    let mut ranges = response.headers().get_all(CONTENT_RANGE).iter();
    let header = ranges
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| WorkshopError::IntegrityMismatch("206 响应缺少 Content-Range 头".into()))?;
    if ranges.next().is_some() {
        return Err(WorkshopError::IntegrityMismatch(
            "206 响应包含多个 Content-Range 头".into(),
        ));
    }
    let (start, end_exclusive, total) = parse_content_range(header)
        .ok_or_else(|| WorkshopError::IntegrityMismatch(format!("无效 Content-Range: {header}")))?;
    if start != offset {
        return Err(WorkshopError::IntegrityMismatch(format!(
            "续传偏移不一致: 期望 {offset}, 实际 {start}"
        )));
    }
    if response
        .content_length()
        .is_some_and(|len| len != end_exclusive - start)
    {
        return Err(WorkshopError::IntegrityMismatch(
            "Content-Length 与 Content-Range 声明的长度不一致".into(),
        ));
    }
    if previous_total
        .zip(total)
        .is_some_and(|(old, new)| old != new)
    {
        return Err(WorkshopError::IntegrityMismatch(
            "远端总文件大小已变化".into(),
        ));
    }
    let total = total
        .or(previous_total)
        .ok_or_else(|| WorkshopError::IntegrityMismatch("206 响应缺少可校验的文件总长度".into()))?;
    if end_exclusive > total {
        return Err(WorkshopError::IntegrityMismatch(
            "Content-Range 超出文件总长度".into(),
        ));
    }
    Ok((Some(total), Some(end_exclusive)))
}

fn same_download_entry(previous: &str, current: &str) -> bool {
    previous == current
        || url_without_signature_values(previous)
            .is_some_and(|previous| Some(previous) == url_without_signature_values(current))
}

fn resource_fingerprints(resolved: &ResolvedRequest) -> (String, Option<String>) {
    let url = resolved
        .url
        .split_once('#')
        .map_or(resolved.url.as_str(), |(url, _)| url);
    let headers: BTreeMap<_, _> = resolved.headers.iter().collect();
    // The effective route may contain a signed URL or credentials in its headers.
    let hash_url = |url: &str| {
        let resource = serde_json::to_vec(&(url, &resolved.original_host, &headers))
            .expect("download resource is serializable");
        format!("{:x}", Sha256::digest(resource))
    };
    (
        hash_url(url),
        url_without_signature_values(url).as_deref().map(hash_url),
    )
}

fn url_without_signature_values(url: &str) -> Option<String> {
    const SIGNING_PARAMETERS: &[(&str, &[&str])] = &[
        ("X-Amz-Signature", &["X-Amz-Date", "X-Amz-Expires"]),
        ("X-Goog-Signature", &["X-Goog-Date", "X-Goog-Expires"]),
        ("Signature", &["Expires"]),
        ("signature", &["expires"]),
        ("sig", &["expires"]),
    ];
    let url = url.split_once('#').map_or(url, |(url, _)| url);
    let (base, query) = url.split_once('?')?;
    if query.contains(';') {
        return None;
    }
    let (signature, volatile_fields) =
        SIGNING_PARAMETERS.iter().copied().find(|(signature, _)| {
            let mut signatures = query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .filter_map(|(key, value)| (key == *signature).then_some(value));
            signatures.next().is_some_and(|value| !value.is_empty()) && signatures.next().is_none()
        })?;
    // Preserve the spelling and order of the other fields, including any object version.
    let query = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if key == signature || volatile_fields.contains(&key) => {
                &pair[..key.len() + 1]
            }
            _ => pair,
        })
        .collect::<Vec<_>>()
        .join("&");
    Some(format!("{base}?{query}"))
}

fn is_strong_etag(etag: &str) -> bool {
    etag.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .is_some_and(|value| {
            value
                .bytes()
                .all(|byte| byte == b'!' || (b'#'..=b'~').contains(&byte))
        })
}

fn calc_percent(downloaded: u64, total_size: Option<u64>) -> u8 {
    total_size
        .filter(|t| *t > 0)
        .map(|t| (((downloaded as f64 / t as f64) * 100.0) as u8).min(99))
        .unwrap_or(0)
}

fn response_strong_etag(response: &Response) -> Option<&str> {
    let mut values = response.headers().get_all(ETAG).iter();
    let etag = values.next()?.to_str().ok()?.trim();
    (values.next().is_none() && is_strong_etag(etag)).then_some(etag)
}

fn parse_content_range(header: &str) -> Option<(u64, u64, Option<u64>)> {
    let (unit, rest) = header.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range_part, total_part) = rest.split_once('/')?;
    let (start_str, end_str) = range_part.split_once('-')?;
    let start = start_str.trim().parse::<u64>().ok()?;
    let end = end_str.trim().parse::<u64>().ok()?.checked_add(1)?;
    let total = if total_part.trim() == "*" {
        None
    } else {
        Some(total_part.trim().parse::<u64>().ok()?)
    };
    (start < end && total.is_none_or(|total| end <= total)).then_some((start, end, total))
}

fn is_retryable(err: &WorkshopError) -> bool {
    match err {
        WorkshopError::Network(_) | WorkshopError::IntegrityMismatch(_) => true,
        WorkshopError::HttpStatus { status, .. } => *status >= 500 || *status == 429,
        _ => false,
    }
}
