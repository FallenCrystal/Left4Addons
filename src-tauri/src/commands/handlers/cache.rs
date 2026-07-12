use super::*;
use crate::mirrors::MirrorClientExt;
use std::time::Duration;
use tauri::State;

#[tauri::command]
pub async fn get_cache_image(
    image_path: String,
    state: State<'_, crate::AppState>,
) -> Result<Vec<u8>, String> {
    let file_path = resolve_cache_file(&state.cache_dir, &image_path)?;
    fs::read(&file_path).map_err(|e| format!("Failed to read cache image: {}", e))
}

#[tauri::command]
pub async fn cache_remote_image(
    url: String,
    state: State<'_, crate::AppState>,
) -> Result<String, String> {
    let parsed = validate_open_url(&url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only HTTP(S) images can be cached".to_string());
    }

    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(parsed.as_str().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    if let Some(cached_path) = find_cached_remote_image(&state.cache_dir, &hash) {
        return Ok(cached_path);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .get_mirrored(parsed.as_str())
        .send()
        .await
        .map_err(|e| format!("Failed to fetch remote image: {}", e))?;
    if !response.status().is_success() {
        return Err(format!(
            "Remote image responded with status {}",
            response.status()
        ));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let extension = cache_image_extension(content_type.as_deref(), &parsed);
    let filename = format!("{}_remote_image.{}", hash, extension);
    let file_path = state.cache_dir.join(&filename);
    if file_path.exists() {
        return Ok(format!("/cache/{}", filename));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read remote image: {}", e))?;
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create image cache directory: {}", e))?;
    }
    fs::write(&file_path, &bytes).map_err(|e| format!("Failed to cache remote image: {}", e))?;
    Ok(format!("/cache/{}", filename))
}
