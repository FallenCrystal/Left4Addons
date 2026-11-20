use crate::db::{ContributorAuthRecord, ServerRepo};
use axum::http::{HeaderMap, StatusCode};
use sha2::{Digest, Sha256};

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn generate_token() -> String {
    format!(
        "l4a_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let val = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    val.strip_prefix("Bearer ")
        .or_else(|| val.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn verify_admin(
    headers: &HeaderMap,
    expected_admin_token: &str,
) -> Result<(), (StatusCode, String)> {
    if expected_admin_token.trim().is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "服务端未配置有效的管理员令牌".to_string(),
        ));
    }

    let supplied = headers
        .get("X-Admin-Token")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| extract_bearer(headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "缺少管理员令牌".to_string()))?;

    if constant_time_eq(supplied.as_bytes(), expected_admin_token.as_bytes()) {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "管理员令牌无效".to_string()))
    }
}

pub fn verify_contributor(
    headers: &HeaderMap,
    repo: &ServerRepo,
) -> Result<ContributorAuthRecord, (StatusCode, String)> {
    let token = extract_bearer(headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "缺少贡献者 Bearer Token".to_string(),
        )
    })?;
    let hash = hash_token(token);
    let contributor = repo
        .find_contributor_by_token_hash(&hash)
        .map_err(|err| {
            tracing::error!(error = %err, "查询贡献者令牌失败");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "服务端内部错误".to_string(),
            )
        })?
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "贡献者 Token 无效".to_string()))?;

    if !contributor.enabled {
        return Err((StatusCode::FORBIDDEN, "该贡献者已被管理员禁用".to_string()));
    }

    Ok(contributor)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let digest_a = Sha256::digest(a);
    let digest_b = Sha256::digest(b);
    let mut diff = 0u8;
    for (&x, &y) in digest_a.iter().zip(digest_b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
