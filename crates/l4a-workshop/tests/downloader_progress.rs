mod support;

use l4a_mirror::MirrorRouter;
use l4a_protocol::WorkshopId;
use l4a_workshop::{DownloadRequest, ResilientDownloader};
use std::sync::Arc;
use std::time::Duration;
use support::{seed_partial_with_etag_raw, wait_for_progress};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn downloader_refreshes_speed_on_tick_drops_on_stall_and_excludes_resumed_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let resume_from = 4096usize;
    let burst_one = 16 * 1024usize;
    let burst_two = 8 * 1024usize;
    let tail = 64usize;
    let total = resume_from + burst_one + burst_two + tail;
    let (send_sample, mut samples) = mpsc::unbounded_channel();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut req_buf = [0u8; 1024];
        let _ = stream.read(&mut req_buf).await;
        let header = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {resume_from}-{}/{total}\r\nContent-Length: {}\r\nETag: \"speed-v1\"\r\nConnection: close\r\n\r\n",
            total - 1,
            burst_one + burst_two + tail
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        let initial = wait_for_progress(&mut samples, "initial progress", |_| true).await;
        assert_eq!(initial.downloaded, resume_from as u64);
        assert_eq!(initial.speed_bps, Some(0));

        // The first speed sample must exclude the resumed bytes while the body is idle.
        let idle =
            wait_for_progress(&mut samples, "speed before receiving new bytes", |_| true).await;
        assert_eq!(idle.downloaded, resume_from as u64);
        assert_eq!(idle.speed_bps, Some(0));

        // Send the first burst across several chunks and hold the body open during the stall.
        for _ in 0..8 {
            stream.write_all(&vec![0xAB; burst_one / 8]).await.unwrap();
        }
        let first_offset = (resume_from + burst_one) as u64;
        wait_for_progress(&mut samples, "positive speed after the first burst", |p| {
            p.downloaded == first_offset && p.speed_bps.unwrap_or(0) > 0
        })
        .await;
        wait_for_progress(&mut samples, "zero speed while stalled", |p| {
            p.downloaded == first_offset && p.speed_bps == Some(0)
        })
        .await;

        stream.write_all(&vec![0xCD; burst_two]).await.unwrap();
        wait_for_progress(&mut samples, "speed recovery after the second burst", |p| {
            p.downloaded == (resume_from + burst_one + burst_two) as u64
                && p.speed_bps.unwrap_or(0) > 0
        })
        .await;

        stream.write_all(&vec![0xEF; tail]).await.unwrap();
        stream.shutdown().await.unwrap();
        samples
    });

    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let wid = WorkshopId::new(901).unwrap();
    let url = format!("http://{addr}/stream.vpk");
    seed_partial_with_etag_raw(
        &cache_dir,
        wid,
        &url,
        "901.vpk",
        &vec![0x11; resume_from],
        Some(total as u64),
        Some("\"speed-v1\""),
    );

    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()))
        .with_progress_intervals(Duration::from_millis(30), Duration::from_millis(120));

    let dest = dir.path().join("loading/901.vpk");
    let result = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid,
                url,
                dest_path: dest,
                max_retries: 0,
            },
            CancellationToken::new(),
            Some(Arc::new(move |p| {
                let _ = send_sample.send(p);
            })),
        )
        .await;
    let mut samples = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let res = result.unwrap();

    assert_eq!(res.resumed_from, resume_from as u64);
    assert_eq!(res.bytes_written, total as u64);

    let final_sample = std::iter::from_fn(|| samples.try_recv().ok())
        .last()
        .unwrap();
    assert_eq!(final_sample.percent, 100);
}
