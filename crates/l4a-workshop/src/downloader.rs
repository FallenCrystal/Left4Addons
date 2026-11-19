use crate::error::WorkshopError;
use futures_util::StreamExt;
use l4a_mirror::MirrorRouter;
use l4a_protocol::WorkshopId;
use reqwest::header::{CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadProgress {
    pub workshop_id: WorkshopId,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: u8,
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
    dest_file_name: String,
    total_size: Option<u64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

pub struct ResilientDownloader {
    cache_dir: PathBuf,
    mirror: Arc<MirrorRouter>,
    connect_timeout: Duration,
    read_timeout: Duration,
}

impl ResilientDownloader {
    pub fn new(cache_dir: impl AsRef<Path>, mirror: Arc<MirrorRouter>) -> Self {
        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
            mirror,
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(60),
        }
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

        let mut last_err: Option<WorkshopError> = None;
        for attempt in 0..=req.max_retries {
            if cancel.is_cancelled() {
                self.cleanup_partial(req.workshop_id);
                return Err(WorkshopError::Cancelled);
            }

            match self
                .download_once(req, cancel.clone(), on_progress.as_ref())
                .await
            {
                Ok(result) => return Ok(result),
                Err(WorkshopError::Cancelled) => {
                    self.cleanup_partial(req.workshop_id);
                    return Err(WorkshopError::Cancelled);
                }
                Err(err) => {
                    let retryable = is_retryable(&err);
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

        Err(last_err.unwrap_or_else(|| WorkshopError::Network("未知下载错误".into())))
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

        let (client, resolved) = self
            .mirror
            .build_streaming_client_for(&req.url, self.connect_timeout, self.read_timeout)
            .map_err(|e| WorkshopError::Network(e.to_string()))?;

        let mut resume_from = 0u64;
        let mut stored_meta: Option<ResumeMetadata> = None;
        if partial_path.exists() && meta_path.exists() {
            if let Ok(raw) = fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<ResumeMetadata>(&raw) {
                    if meta.workshop_id == req.workshop_id
                        && meta.resolved_url == resolved.url
                        && meta.dest_file_name == dest_name
                    {
                        resume_from = fs::metadata(&partial_path).map(|m| m.len()).unwrap_or(0);
                        stored_meta = Some(meta);
                    } else {
                        self.cleanup_partial(req.workshop_id);
                    }
                }
            }
        }

        let response = if resume_from > 0 {
            let (builder, _) = self.mirror.prepare_get(&client, &req.url);
            let resp = builder
                .header(RANGE, format!("bytes={resume_from}-"))
                .send()
                .await
                .map_err(|e| WorkshopError::Network(e.to_string()))?;
            match resp.status() {
                StatusCode::PARTIAL_CONTENT => resp,
                StatusCode::OK => {
                    self.cleanup_partial(req.workshop_id);
                    resume_from = 0;
                    stored_meta = None;
                    resp
                }
                StatusCode::RANGE_NOT_SATISFIABLE => {
                    self.cleanup_partial(req.workshop_id);
                    resume_from = 0;
                    stored_meta = None;
                    let (fresh_builder, _) = self.mirror.prepare_get(&client, &req.url);
                    fresh_builder
                        .send()
                        .await
                        .map_err(|e| WorkshopError::Network(e.to_string()))?
                }
                _ => resp,
            }
        } else {
            let (builder, _) = self.mirror.prepare_get(&client, &req.url);
            builder
                .send()
                .await
                .map_err(|e| WorkshopError::Network(e.to_string()))?
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(WorkshopError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        let etag = header_str(response.headers(), ETAG);
        let last_modified = header_str(response.headers(), LAST_MODIFIED);

        let total_size = if status == StatusCode::PARTIAL_CONTENT {
            let range_header = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    WorkshopError::IntegrityMismatch("206 响应缺少 Content-Range 头".into())
                })?;
            let (start, total) = parse_content_range(range_header).ok_or_else(|| {
                WorkshopError::IntegrityMismatch(format!("无效 Content-Range: {range_header}"))
            })?;
            if start != resume_from {
                self.cleanup_partial(req.workshop_id);
                return Err(WorkshopError::IntegrityMismatch(format!(
                    "续传偏移不一致: 期望 {resume_from}, 实际 {start}"
                )));
            }
            if let Some(ref prev) = stored_meta {
                if let (Some(a), Some(b)) = (prev.etag.as_deref(), etag.as_deref()) {
                    if a != b {
                        self.cleanup_partial(req.workshop_id);
                        return Err(WorkshopError::IntegrityMismatch("远端 ETag 已变化".into()));
                    }
                }
                if let (Some(a), Some(b)) =
                    (prev.last_modified.as_deref(), last_modified.as_deref())
                {
                    if a != b {
                        self.cleanup_partial(req.workshop_id);
                        return Err(WorkshopError::IntegrityMismatch(
                            "远端 Last-Modified 已变化".into(),
                        ));
                    }
                }
                if let (Some(prev_total), Some(cur_total)) = (prev.total_size, total) {
                    if prev_total != cur_total {
                        self.cleanup_partial(req.workshop_id);
                        return Err(WorkshopError::IntegrityMismatch(
                            "远端总文件大小已变化".into(),
                        ));
                    }
                }
            }
            total
        } else {
            response.content_length()
        };

        let meta = ResumeMetadata {
            workshop_id: req.workshop_id,
            resolved_url: resolved.url,
            dest_file_name: dest_name,
            total_size,
            etag,
            last_modified,
        };
        fs::write(&meta_path, serde_json::to_vec_pretty(&meta).unwrap_or_default())?;

        let mut file = if resume_from > 0 {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&partial_path)?
        } else {
            fs::File::create(&partial_path)?
        };

        let mut downloaded = resume_from;
        let mut stream = response.bytes_stream();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    drop(file);
                    return Err(WorkshopError::Cancelled);
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            file.write_all(&bytes)?;
                            downloaded += bytes.len() as u64;
                            if let Some(sink) = on_progress {
                                let percent = total_size
                                    .filter(|t| *t > 0)
                                    .map(|t| (((downloaded as f64 / t as f64) * 100.0) as u8).min(99))
                                    .unwrap_or(0);
                                sink(DownloadProgress {
                                    workshop_id: req.workshop_id,
                                    downloaded,
                                    total: total_size,
                                    percent,
                                });
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
            });
        }

        Ok(DownloadResult {
            dest_path: req.dest_path.clone(),
            bytes_written: downloaded,
            resumed_from: resume_from,
        })
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

fn header_str(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn parse_content_range(header: &str) -> Option<(u64, Option<u64>)> {
    let rest = header.trim().strip_prefix("bytes ")?;
    let (range_part, total_part) = rest.split_once('/')?;
    let (start_str, _end_str) = range_part.split_once('-')?;
    let start = start_str.trim().parse::<u64>().ok()?;
    let total = if total_part.trim() == "*" {
        None
    } else {
        total_part.trim().parse::<u64>().ok()
    };
    Some((start, total))
}

fn is_retryable(err: &WorkshopError) -> bool {
    match err {
        WorkshopError::Network(_) | WorkshopError::IntegrityMismatch(_) => true,
        WorkshopError::HttpStatus { status, .. } => *status >= 500 || *status == 429,
        _ => false,
    }
}
