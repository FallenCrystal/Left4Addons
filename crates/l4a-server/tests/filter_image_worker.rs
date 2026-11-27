#![cfg(any(unix, windows))]

use image::{ImageFormat, Rgb, RgbImage};
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

#[test]
fn webp_worker_resizes_the_maximum_lossless_edge() {
    let mut webp = Cursor::new(Vec::new());
    RgbImage::from_pixel(16_384, 48, Rgb([60, 120, 150]))
        .write_to(&mut webp, ImageFormat::WebP)
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_l4a-server"))
        .arg("--l4a-filter-webp-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(webp.get_ref())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "worker exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        image::guess_format(&output.stdout).unwrap(),
        ImageFormat::Jpeg
    );
    let jpeg = image::load_from_memory(&output.stdout).unwrap();
    assert_eq!((jpeg.width(), jpeg.height()), (4096, 12));
}
