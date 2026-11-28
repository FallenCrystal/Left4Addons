use crate::error::WorkshopError;
use l4a_mirror::ResolvedRequest;
use l4a_protocol::WorkshopId;
use reqwest::header::{CONTENT_RANGE, ETAG};
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResumeMetadata {
    pub(super) workshop_id: WorkshopId,
    pub(super) resolved_url: String,
    pub(super) resource_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) signed_resource_fingerprint: Option<String>,
    pub(super) dest_file_name: String,
    pub(super) total_size: Option<u64>,
    pub(super) etag: Option<String>,
}

impl ResumeMetadata {
    pub(super) fn matches_resource(
        &self,
        fingerprint: &str,
        signed_fingerprint: Option<&str>,
    ) -> bool {
        self.resource_fingerprint.as_deref() == Some(fingerprint)
            || (signed_fingerprint.is_some()
                && self.signed_resource_fingerprint.as_deref() == signed_fingerprint)
    }

    pub(super) fn strong_etag(&self) -> Option<&str> {
        self.etag.as_deref().filter(|etag| is_strong_etag(etag))
    }
}

pub(super) fn load_resume_state(
    meta_path: &Path,
    partial_path: &Path,
    workshop_id: WorkshopId,
    dest_name: &str,
    resolved_url: &str,
) -> Option<(u64, ResumeMetadata)> {
    let stored_meta = fs::read(meta_path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<ResumeMetadata>(&raw).ok())
        .filter(|meta| {
            meta.workshop_id == workshop_id
                && meta.dest_file_name == dest_name
                && meta.resource_fingerprint.is_some()
                && meta.strong_etag().is_some()
                && same_download_entry(&meta.resolved_url, resolved_url)
        })?;
    let offset = fs::metadata(partial_path).ok()?.len();
    (offset > 0).then_some((offset, stored_meta))
}

pub(super) fn response_lengths(
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

pub(super) fn resource_fingerprints(resolved: &ResolvedRequest) -> (String, Option<String>) {
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

pub(super) fn response_strong_etag(response: &Response) -> Option<&str> {
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

pub(super) fn calc_percent(downloaded: u64, total_size: Option<u64>) -> u8 {
    total_size
        .filter(|t| *t > 0)
        .map(|t| (((downloaded as f64 / t as f64) * 100.0) as u8).min(99))
        .unwrap_or(0)
}

pub(super) fn is_retryable(err: &WorkshopError) -> bool {
    match err {
        WorkshopError::Network(_) | WorkshopError::IntegrityMismatch(_) => true,
        WorkshopError::HttpStatus { status, .. } => *status >= 500 || *status == 429,
        _ => false,
    }
}
