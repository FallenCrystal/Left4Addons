use axum::http::{header, HeaderMap, StatusCode};
use chrono::{DateTime, Duration, Utc};
use l4a_protocol::{AdminLoginResponse, AdminSessionInfo};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::RwLock;

pub const ADMIN_SESSION_COOKIE: &str = "l4a_admin_session";
pub type AuthRejection = (StatusCode, String);
type VerifiedAdmin = (String, Option<DateTime<Utc>>);

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

#[derive(Debug, Clone)]
struct SessionRecord {
    username: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug)]
struct AuthState {
    username: String,
    password: String,
    session_ttl_hours: u64,
    sessions: HashMap<String, SessionRecord>,
}

pub struct AdminAuthManager {
    inner: RwLock<AuthState>,
}

impl AdminAuthManager {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        session_ttl_hours: u64,
    ) -> Self {
        Self {
            inner: RwLock::new(AuthState {
                username: username.into(),
                password: password.into(),
                session_ttl_hours: session_ttl_hours.max(1),
                sessions: HashMap::new(),
            }),
        }
    }

    pub fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<AdminLoginResponse, (StatusCode, String)> {
        let mut guard = self.inner.write().unwrap();
        if guard.password.trim().is_empty() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "服务端未配置有效的管理员密码".to_string(),
            ));
        }

        let user_ok = constant_time_eq(username.trim().as_bytes(), guard.username.as_bytes());
        let pass_ok = constant_time_eq(password.as_bytes(), guard.password.as_bytes());
        if !user_ok || !pass_ok {
            return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
        }

        let now = Utc::now();
        guard.sessions.retain(|_, s| s.expires_at > now);

        let raw_token = generate_token();
        let expires_at = now + Duration::hours(guard.session_ttl_hours as i64);
        let canonical_user = guard.username.clone();
        guard.sessions.insert(
            hash_token(&raw_token),
            SessionRecord {
                username: canonical_user.clone(),
                expires_at,
            },
        );

        Ok(AdminLoginResponse {
            token: raw_token,
            username: canonical_user,
            expires_at,
        })
    }

    pub fn logout(&self, headers: &HeaderMap) {
        let mut guard = self.inner.write().unwrap();
        for token in extract_candidate_tokens(headers) {
            guard.sessions.remove(&hash_token(&token));
        }
    }

    pub fn session_info(&self, headers: &HeaderMap) -> AdminSessionInfo {
        match self.verify_headers(headers) {
            Ok((username, expires_at)) => AdminSessionInfo {
                authenticated: true,
                username: Some(username),
                expires_at,
            },
            Err(_) => AdminSessionInfo {
                authenticated: false,
                username: None,
                expires_at: None,
            },
        }
    }

    pub fn verify_admin(&self, headers: &HeaderMap) -> Result<String, AuthRejection> {
        self.verify_headers(headers).map(|(user, _)| user)
    }

    fn verify_headers(&self, headers: &HeaderMap) -> Result<VerifiedAdmin, AuthRejection> {
        let guard = self.inner.read().unwrap();
        if guard.password.trim().is_empty() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "服务端未配置有效的管理员密码".to_string(),
            ));
        }

        let candidates = extract_candidate_tokens(headers);
        if candidates.is_empty() {
            return Err((
                StatusCode::UNAUTHORIZED,
                "未登录或缺少管理员凭证".to_string(),
            ));
        }

        let now = Utc::now();
        for supplied in candidates {
            if let Some(session) = guard.sessions.get(&hash_token(&supplied)) {
                if session.expires_at > now {
                    return Ok((session.username.clone(), Some(session.expires_at)));
                }
            }
            if constant_time_eq(supplied.as_bytes(), guard.password.as_bytes()) {
                return Ok((guard.username.clone(), None));
            }
        }

        Err((
            StatusCode::FORBIDDEN,
            "管理员会话已过期或凭证无效".to_string(),
        ))
    }

    pub fn change_password(
        &self,
        current_password: &str,
        new_password: &str,
        keep_headers: &HeaderMap,
    ) -> Result<String, (StatusCode, String)> {
        let trimmed_new = new_password.trim();
        if trimmed_new.len() < 8 {
            return Err((
                StatusCode::BAD_REQUEST,
                "新管理员密码长度至少需要 8 个字符".to_string(),
            ));
        }

        let mut guard = self.inner.write().unwrap();
        if !constant_time_eq(current_password.as_bytes(), guard.password.as_bytes()) {
            return Err((StatusCode::FORBIDDEN, "当前密码不正确".to_string()));
        }

        let keep_hashes: Vec<String> = extract_candidate_tokens(keep_headers)
            .into_iter()
            .map(|t| hash_token(&t))
            .collect();
        guard.password = trimmed_new.to_string();
        guard
            .sessions
            .retain(|k, s| keep_hashes.contains(k) && s.expires_at > Utc::now());
        Ok(guard.username.clone())
    }

    pub fn session_ttl_secs(&self) -> u64 {
        self.inner.read().unwrap().session_ttl_hours * 3600
    }
}

pub fn build_session_cookie(token: &str, max_age_secs: u64) -> String {
    format!(
        "{ADMIN_SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}"
    )
}

pub fn build_clear_session_cookie() -> String {
    format!("{ADMIN_SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

fn extract_candidate_tokens(headers: &HeaderMap) -> Vec<String> {
    let mut out = Vec::new();

    if let Some(raw) = headers.get("X-Admin-Token").and_then(|v| v.to_str().ok()) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }

    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push(token.to_string());
        }
    }

    for cookie_val in headers.get_all(header::COOKIE) {
        let Ok(cookie_str) = cookie_val.to_str() else {
            continue;
        };
        for pair in cookie_str.split(';') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                if k.trim() == ADMIN_SESSION_COOKIE {
                    let token = v.trim();
                    if !token.is_empty() {
                        out.push(token.to_string());
                    }
                }
            }
        }
    }

    out
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
