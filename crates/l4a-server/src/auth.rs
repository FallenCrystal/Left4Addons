use axum::http::{header, HeaderMap, StatusCode};
use chrono::{DateTime, Duration, Utc};
use l4a_protocol::{AdminLoginResponse, AdminSessionInfo};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::RwLock;

pub const ADMIN_SESSION_COOKIE: &str = "l4a_admin_session";
pub type AuthRejection = (StatusCode, String);

struct VerifiedAdmin {
    username: String,
    expires_at: Option<DateTime<Utc>>,
    cookie_csrf_token: Option<String>,
}

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
            guard.sessions.remove(&hash_token(token));
        }
    }

    pub fn session_info(&self, headers: &HeaderMap) -> AdminSessionInfo {
        match self.verify_headers(headers) {
            Ok(admin) => AdminSessionInfo {
                authenticated: true,
                username: Some(admin.username),
                expires_at: admin.expires_at,
                csrf_token: admin.cookie_csrf_token,
            },
            Err(_) => AdminSessionInfo {
                authenticated: false,
                username: None,
                expires_at: None,
                csrf_token: None,
            },
        }
    }

    pub fn verify_admin(&self, headers: &HeaderMap) -> Result<String, AuthRejection> {
        self.verify_headers(headers).map(|admin| admin.username)
    }

    pub(crate) fn allows_admin_write(&self, headers: &HeaderMap) -> bool {
        self.verify_headers(headers)
            .ok()
            .and_then(|admin| admin.cookie_csrf_token)
            .is_none_or(|expected| {
                headers
                    .get("X-L4A-CSRF")
                    .is_some_and(|value| constant_time_eq(value.as_bytes(), expected.as_bytes()))
            })
    }

    fn verify_headers(&self, headers: &HeaderMap) -> Result<VerifiedAdmin, AuthRejection> {
        let guard = self.inner.read().unwrap();
        if guard.password.trim().is_empty() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "服务端未配置有效的管理员密码".to_string(),
            ));
        }

        let mut candidates = explicit_tokens(headers)
            .map(|token| (token, false))
            .chain(cookie_tokens(headers).map(|token| (token, true)))
            .peekable();
        if candidates.peek().is_none() {
            return Err((
                StatusCode::UNAUTHORIZED,
                "未登录或缺少管理员凭证".to_string(),
            ));
        }

        let now = Utc::now();
        for (supplied, is_cookie) in candidates {
            if let Some(session) = guard.sessions.get(&hash_token(supplied)) {
                if session.expires_at > now {
                    return Ok(VerifiedAdmin {
                        username: session.username.clone(),
                        expires_at: Some(session.expires_at),
                        cookie_csrf_token: is_cookie.then(|| csrf_token(supplied)),
                    });
                }
            }
            if !is_cookie && constant_time_eq(supplied.as_bytes(), guard.password.as_bytes()) {
                return Ok(VerifiedAdmin {
                    username: guard.username.clone(),
                    expires_at: None,
                    cookie_csrf_token: None,
                });
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
        persist: impl FnOnce(&str) -> Result<(), (StatusCode, String)>,
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

        persist(trimmed_new)?;

        let keep_hashes: Vec<String> = extract_candidate_tokens(keep_headers)
            .map(hash_token)
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

pub fn build_session_cookie(token: &str, max_age_secs: u64, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{ADMIN_SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}{secure_attr}"
    )
}

pub fn build_clear_session_cookie(secure: bool) -> String {
    build_session_cookie("", 0, secure)
}

fn extract_candidate_tokens(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    explicit_tokens(headers).chain(cookie_tokens(headers))
}

fn explicit_tokens(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    let admin_token = headers.get("X-Admin-Token").and_then(|v| v.to_str().ok());
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| {
            auth.strip_prefix("Bearer ")
                .or_else(|| auth.strip_prefix("bearer "))
        });
    admin_token
        .into_iter()
        .chain(bearer)
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn cookie_tokens(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    headers
        .get_all(header::COOKIE)
        .into_iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|cookie| cookie.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(key, _)| key.trim() == ADMIN_SESSION_COOKIE)
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
}

fn csrf_token(session_token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"l4a-admin-csrf\0");
    digest.update(session_token.as_bytes());
    hex::encode(digest.finalize())
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
