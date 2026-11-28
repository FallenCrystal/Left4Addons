#![allow(dead_code)]

use axum::Router;
use l4a_protocol::WorkshopId;
use l4a_workshop::{DownloadProgress, DownloadRequest};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub async fn start_router(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

pub fn direct_resource_fingerprint(url: &str) -> String {
    let resource = serde_json::to_vec(&json!([url, null, {}])).unwrap();
    format!("{:x}", Sha256::digest(resource))
}

pub fn seed_partial(
    cache: &Path,
    request: &DownloadRequest,
    bytes: &[u8],
    total_size: Option<u64>,
    etag: Option<&str>,
) {
    let id = request.workshop_id;
    std::fs::create_dir_all(cache).unwrap();
    std::fs::write(cache.join(format!("{id}.vpk.part")), bytes).unwrap();
    std::fs::write(
        cache.join(format!("{id}.part.meta.json")),
        serde_json::to_vec(&json!({
            "workshopId": id,
            "resolvedUrl": request.url,
            "resourceFingerprint": direct_resource_fingerprint(&request.url),
            "destFileName": request.dest_path.file_name().unwrap().to_str().unwrap(),
            "totalSize": total_size,
            "etag": etag,
        }))
        .unwrap(),
    )
    .unwrap();
}

pub fn seed_partial_with_etag_raw(
    cache: &Path,
    wid: WorkshopId,
    url: &str,
    dest_file_name: &str,
    bytes: &[u8],
    total_size: Option<u64>,
    etag: Option<&str>,
) {
    std::fs::create_dir_all(cache).unwrap();
    std::fs::write(cache.join(format!("{wid}.vpk.part")), bytes).unwrap();
    std::fs::write(
        cache.join(format!("{wid}.part.meta.json")),
        serde_json::to_vec(&json!({
            "workshopId": wid.to_string(),
            "resolvedUrl": url,
            "resourceFingerprint": direct_resource_fingerprint(url),
            "destFileName": dest_file_name,
            "totalSize": total_size,
            "etag": etag,
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();
}

pub async fn wait_for_progress(
    samples: &mut mpsc::UnboundedReceiver<DownloadProgress>,
    description: &str,
    matches: impl Fn(&DownloadProgress) -> bool,
) -> DownloadProgress {
    timeout(Duration::from_secs(5), async {
        while let Some(sample) = samples.recv().await {
            if matches(&sample) {
                return sample;
            }
        }
        panic!("progress stream ended before {description}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

pub async fn start_counting_keep_alive_server<F>(handler: F) -> (String, Arc<Mutex<Vec<usize>>>)
where
    F: Fn(&str, &str) -> (u16, &'static str, Vec<u8>) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let next_conn_id = Arc::new(AtomicUsize::new(1));
    let served_conns = Arc::new(Mutex::new(Vec::<usize>::new()));
    let handler = Arc::new(handler);

    let served_conns_bg = Arc::clone(&served_conns);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let conn_id = next_conn_id.fetch_add(1, Ordering::SeqCst);
            let served_conn = Arc::clone(&served_conns_bg);
            let handler_conn = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut pending = Vec::new();
                loop {
                    let header_end = loop {
                        if let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                            break Some(pos + 4);
                        }
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => break None,
                            Ok(n) => pending.extend_from_slice(&buf[..n]),
                        }
                    };
                    let Some(end) = header_end else {
                        break;
                    };
                    let header_text = String::from_utf8_lossy(&pending[..end]).into_owned();
                    let mut lines = header_text.lines();
                    let request_line = lines.next().unwrap_or_default();
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();
                    let content_length = lines
                        .find_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    pending.drain(..end);
                    while pending.len() < content_length {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => pending.extend_from_slice(&buf[..n]),
                        }
                    }
                    pending.drain(..content_length);

                    served_conn.lock().unwrap().push(conn_id);
                    let (status, content_type, body) = handler_conn(&method, &path);
                    let reason = match status {
                        200 => "OK",
                        404 => "Not Found",
                        416 => "Range Not Satisfiable",
                        _ => "Status",
                    };
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                        body.len()
                    );
                    if socket.write_all(head.as_bytes()).await.is_err()
                        || socket.write_all(&body).await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });

    (format!("http://{addr}"), served_conns)
}
