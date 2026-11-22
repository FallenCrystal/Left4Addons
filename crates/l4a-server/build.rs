use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                out.push((rel_str, path));
            }
        }
    }
}

fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".json") || path.ends_with(".map") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let web_dist = manifest_dir.join("web-dist");
    println!("cargo:rerun-if-changed={}", web_dist.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let dest = out_dir.join("web_assets_gen.rs");

    let mut files = Vec::new();
    if web_dist.join("index.html").is_file() {
        collect_files(&web_dist, &web_dist, &mut files);
    }

    let mut code = String::from("pub static WEB_ASSETS: &[(&str, &str, &[u8])] = &[\n");
    if files.is_empty() {
        let fallback_html = concat!(
            "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"/>",
            "<title>Left4Addons Catalog Console</title></head>",
            "<body><div id=\"root\">Left4Addons Catalog Console (run `npm run catalog:build` to bundle frontend assets)</div></body></html>"
        );
        code.push_str(&format!(
            "    (\"index.html\", \"text/html; charset=utf-8\", b{fallback_html:?}),\n"
        ));
    } else {
        for (rel, abs) in files {
            let mime = mime_for(&rel);
            let abs_escaped = abs.to_string_lossy().replace('\\', "/");
            code.push_str(&format!(
                "    ({rel:?}, {mime:?}, include_bytes!({abs_escaped:?})),\n"
            ));
        }
    }
    code.push_str("];\n");

    fs::write(dest, code).expect("failed to write web_assets_gen.rs");
}
