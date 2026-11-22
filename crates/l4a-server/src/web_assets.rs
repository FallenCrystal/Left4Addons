use axum::{
    extract::Path,
    http::{
        header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS},
        HeaderValue, StatusCode,
    },
    response::{IntoResponse, Redirect, Response},
};

include!(concat!(env!("OUT_DIR"), "/web_assets_gen.rs"));

const CSP_HEADER: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' https: data:; connect-src 'self'; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

pub async fn redirect_root_to_admin() -> Redirect {
    Redirect::temporary("/admin/")
}

pub async fn serve_admin_index() -> Response {
    serve_asset_or_spa("index.html")
}

pub async fn serve_admin_asset(Path(path): Path<String>) -> Response {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return serve_asset_or_spa("index.html");
    }
    serve_asset_or_spa(trimmed)
}

fn serve_asset_or_spa(rel_path: &str) -> Response {
    if let Some((_, mime, bytes)) = WEB_ASSETS.iter().find(|(p, _, _)| *p == rel_path) {
        let cache = if rel_path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return build_asset_response(StatusCode::OK, mime, cache, bytes);
    }

    // 对未知静态资源后缀直接返回 404，其余前端路由回退到 SPA index.html
    if rel_path.starts_with("assets/") || rel_path.contains('.') {
        return (StatusCode::NOT_FOUND, "Asset not found").into_response();
    }

    if let Some((_, mime, bytes)) = WEB_ASSETS.iter().find(|(p, _, _)| *p == "index.html") {
        return build_asset_response(StatusCode::OK, mime, "no-cache", bytes);
    }

    (StatusCode::SERVICE_UNAVAILABLE, "Admin UI unavailable").into_response()
}

fn build_asset_response(
    status: StatusCode,
    mime: &'static str,
    cache_control: &'static str,
    bytes: &'static [u8],
) -> Response {
    (
        status,
        [
            (CONTENT_TYPE, HeaderValue::from_static(mime)),
            (CACHE_CONTROL, HeaderValue::from_static(cache_control)),
            (
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CSP_HEADER),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        ],
        bytes,
    )
        .into_response()
}
