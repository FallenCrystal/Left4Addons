use regex::Regex;
use reqwest::{Client, ClientBuilder, RequestBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MirrorError {
    #[error("读取镜像配置文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("解析镜像配置 JSON 失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("构建 HTTP 客户端失败: {0}")]
    ClientBuild(#[from] reqwest::Error),
    #[error("已启用仅 HTTPS 连接，请使用 HTTPS 地址")]
    HttpsRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MirrorRule {
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub subdomains: Option<Vec<String>>,
    #[serde(rename = "allow-www", default)]
    pub allow_www: bool,
    #[serde(default)]
    pub regex: Option<String>,
    pub target: String,
    #[serde(rename = "keep-host", default)]
    pub keep_host: bool,
    #[serde(rename = "insecure-tls", default)]
    pub insecure_tls: bool,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    regex: Regex,
    raw: MirrorRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub url: String,
    pub original_host: Option<String>,
    pub headers: HashMap<String, String>,
    pub insecure_tls: bool,
}

/// 可注入、无全局状态的镜像路由器。
///
/// 规则配置加载时即预编译正则表达式，彻底消除每次请求重复编译与全局 `OnceLock` 副作用。
#[derive(Debug, Clone, Default)]
pub struct MirrorRouter {
    rules: Vec<CompiledRule>,
    https_only: Arc<AtomicBool>,
}

impl MirrorRouter {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_rules(rules: Vec<MirrorRule>) -> Self {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let Some(pattern) = rule_pattern(&rule) else {
                continue;
            };
            match Regex::new(&pattern) {
                Ok(regex) => compiled.push(CompiledRule { regex, raw: rule }),
                Err(err) => {
                    tracing::warn!(pattern = %pattern, error = %err, "忽略无效镜像规则正则");
                }
            }
        }
        Self {
            rules: compiled,
            ..Self::default()
        }
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, MirrorError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::empty());
        }
        let content = fs::read_to_string(path)?;
        let rules: Vec<MirrorRule> = serde_json::from_str(&content)?;
        Ok(Self::from_rules(rules))
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn set_https_only(&self, enabled: bool) {
        self.https_only.store(enabled, Ordering::Relaxed);
    }

    pub fn resolve(&self, url: &str) -> ResolvedRequest {
        if self.rules.is_empty() {
            return ResolvedRequest {
                url: url.to_string(),
                original_host: None,
                headers: HashMap::new(),
                insecure_tls: false,
            };
        }

        let Ok(mut parsed_url) = url::Url::parse(url) else {
            return ResolvedRequest {
                url: url.to_string(),
                original_host: None,
                headers: HashMap::new(),
                insecure_tls: false,
            };
        };

        let Some(host) = parsed_url
            .host_str()
            .map(|hostname| match parsed_url.port() {
                Some(port) => format!("{hostname}:{port}"),
                None => hostname.to_string(),
            })
        else {
            return ResolvedRequest {
                url: url.to_string(),
                original_host: None,
                headers: HashMap::new(),
                insecure_tls: false,
            };
        };

        for compiled in &self.rules {
            let Some(captures) = compiled.regex.captures(&host) else {
                continue;
            };
            let target = replace_captures(&compiled.raw.target, &captures);
            if set_target_host(&mut parsed_url, &target) {
                return ResolvedRequest {
                    url: parsed_url.to_string(),
                    original_host: compiled.raw.keep_host.then_some(host),
                    headers: compiled.raw.headers.clone().unwrap_or_default(),
                    insecure_tls: compiled.raw.insecure_tls,
                };
            }
        }

        ResolvedRequest {
            url: url.to_string(),
            original_host: None,
            headers: HashMap::new(),
            insecure_tls: false,
        }
    }

    pub fn build_client_for(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(Client, ResolvedRequest), MirrorError> {
        let resolved = self.resolve(url);
        let mut builder = self.client_builder(&resolved)?;
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        let client = builder.build()?;
        Ok((client, resolved))
    }

    pub fn build_streaming_client_for(
        &self,
        url: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<(Client, ResolvedRequest), MirrorError> {
        let resolved = self.resolve(url);
        let client = self
            .client_builder(&resolved)?
            .connect_timeout(connect_timeout)
            .read_timeout(read_timeout)
            .build()?;
        Ok((client, resolved))
    }

    fn client_builder(&self, resolved: &ResolvedRequest) -> Result<ClientBuilder, MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        if https_only && !url::Url::parse(&resolved.url).is_ok_and(|url| url.scheme() == "https") {
            return Err(MirrorError::HttpsRequired);
        }
        let insecure_tls = resolved.insecure_tls && !https_only;
        if insecure_tls {
            tracing::warn!("镜像规则启用了 insecure-tls 跳过证书校验");
        }
        Ok(Client::builder()
            .https_only(https_only)
            .danger_accept_invalid_certs(insecure_tls))
    }

    pub fn prepare_get(&self, client: &Client, url: &str) -> (RequestBuilder, ResolvedRequest) {
        let resolved = self.resolve(url);
        let req = apply_resolved_headers(client.get(&resolved.url), &resolved);
        (req, resolved)
    }

    pub fn prepare_post(&self, client: &Client, url: &str) -> (RequestBuilder, ResolvedRequest) {
        let resolved = self.resolve(url);
        let req = apply_resolved_headers(client.post(&resolved.url), &resolved);
        (req, resolved)
    }
}

fn apply_resolved_headers(mut req: RequestBuilder, resolved: &ResolvedRequest) -> RequestBuilder {
    if let Some(ref host) = resolved.original_host {
        req = req.header(reqwest::header::HOST, host);
    }
    for (k, v) in &resolved.headers {
        req = req.header(k, v);
    }
    req
}

fn rule_pattern(rule: &MirrorRule) -> Option<String> {
    if let Some(regex) = &rule.regex {
        return Some(regex.clone());
    }

    if let Some(domain) = &rule.domain {
        let mut pattern = String::from("^");
        if rule.allow_www {
            pattern.push_str(r"(?:www\.)?");
        }
        if let Some(subdomains) = &rule.subdomains {
            if subdomains.len() == 1 && subdomains[0] == "*" {
                pattern.push_str(r"(.*)\.");
            } else if !subdomains.is_empty() {
                let alternatives = subdomains
                    .iter()
                    .map(|subdomain| regex::escape(subdomain))
                    .collect::<Vec<_>>();
                pattern.push_str(&format!("({})\\.", alternatives.join("|")));
            }
        }
        pattern.push_str(&regex::escape(domain));
        pattern.push('$');
        return Some(pattern);
    }

    rule.rules.as_deref().and_then(string_rule_pattern)
}

fn string_rule_pattern(rule: &str) -> Option<String> {
    let patterns = rule
        .split(';')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .map(string_rule_pattern_fragment)
        .collect::<Vec<_>>();

    match patterns.as_slice() {
        [] => None,
        [pattern] => Some(format!("^{pattern}$")),
        _ => Some(format!("^(?:{})$", patterns.join("|"))),
    }
}

fn string_rule_pattern_fragment(rule: &str) -> String {
    let mut pattern = String::new();
    let mut in_group = false;
    let mut group = String::new();
    let chars: Vec<char> = rule.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '[' => {
                in_group = true;
                pattern.push('(');
            }
            ']' => {
                in_group = false;
                pattern.push_str(
                    &group
                        .split(',')
                        .map(regex::escape)
                        .collect::<Vec<_>>()
                        .join("|"),
                );
                group.clear();
                pattern.push(')');
                if chars.get(index + 1) == Some(&'?') {
                    pattern.push('?');
                    index += 1;
                }
            }
            character if in_group => group.push(character),
            '*' => pattern.push_str("(.*?)"),
            character => pattern.push_str(&regex::escape(&character.to_string())),
        }
        index += 1;
    }

    pattern
}

fn replace_captures(target: &str, captures: &regex::Captures<'_>) -> String {
    let mut replaced = target.to_string();
    for (index, capture) in captures.iter().enumerate().skip(1) {
        if let Some(capture) = capture {
            replaced = replaced.replace(&format!("${index}"), capture.as_str());
        }
    }
    if replaced.contains('*') {
        let first_capture = captures.get(1).map_or("", |capture| capture.as_str());
        replaced = replaced.replace('*', first_capture);
    }
    replaced
}

fn set_target_host(url: &mut url::Url, target: &str) -> bool {
    let Ok(target_url) = url::Url::parse(&format!("l4a://{target}")) else {
        return false;
    };
    let Some(target_host) = target_url.host_str() else {
        return false;
    };
    if url.set_host(Some(target_host)).is_err() {
        return false;
    }
    if url.set_port(target_url.port()).is_err() {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn base_rule(target: &str) -> MirrorRule {
        MirrorRule {
            rules: None,
            domain: None,
            subdomains: None,
            allow_www: false,
            regex: None,
            target: target.to_string(),
            keep_host: false,
            insecure_tls: false,
            headers: None,
        }
    }

    #[test]
    fn object_rule_rewrites_subdomain_host_and_headers() {
        let mut r = base_rule("$1.mirror.example.com");
        r.domain = Some("steamstatic.com".to_string());
        r.subdomains = Some(vec!["store".to_string(), "media".to_string()]);
        r.allow_www = true;
        r.keep_host = true;
        r.insecure_tls = true;
        r.headers = Some(HashMap::from([("X-Mirror".into(), "on".into())]));

        let router = MirrorRouter::from_rules(vec![r]);
        let res = router.resolve("https://www.media.steamstatic.com/img.jpg?v=1");
        assert_eq!(res.url, "https://media.mirror.example.com/img.jpg?v=1");
        assert_eq!(
            res.original_host.as_deref(),
            Some("www.media.steamstatic.com")
        );
        assert_eq!(res.headers.get("X-Mirror").map(String::as_str), Some("on"));
        assert!(res.insecure_tls);
    }

    #[test]
    fn string_rule_handles_optional_group_wildcard_and_port() {
        let mut r = base_rule("$2.proxy.local:8443");
        r.rules = Some("[www.]?*.steamcommunity.com".to_string());

        let router = MirrorRouter::from_rules(vec![r]);
        let res = router.resolve("https://www.workshop.steamcommunity.com/sharedfiles?id=1");
        assert_eq!(
            res.url,
            "https://workshop.proxy.local:8443/sharedfiles?id=1"
        );
        assert_eq!(res.original_host, None);
    }

    #[test]
    fn semicolon_rules_and_invalid_regex_are_handled_gracefully() {
        let mut invalid = base_rule("broken.local");
        invalid.regex = Some("[unclosed".to_string());

        let mut multi = base_rule("cdn.mirror.local");
        multi.rules = Some("cdn.steamstatic.com; community.steamstatic.com".to_string());

        let router = MirrorRouter::from_rules(vec![invalid, multi]);
        assert_eq!(router.rule_count(), 1);
        assert_eq!(
            router.resolve("https://community.steamstatic.com/a").url,
            "https://cdn.mirror.local/a"
        );
        assert_eq!(
            router.resolve("https://untouched.example.com/b").url,
            "https://untouched.example.com/b"
        );
    }

    #[test]
    fn loads_rules_from_json_file_and_handles_missing_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert_eq!(MirrorRouter::from_file(&missing).unwrap().rule_count(), 0);

        let file = dir.path().join("mirrors.json");
        std::fs::write(
            &file,
            r#"[{"domain":"steampowered.com","subdomains":["api"],"target":"127.0.0.1:9000","keep-host":true}]"#,
        )
        .unwrap();

        let router = MirrorRouter::from_file(&file).unwrap();
        assert_eq!(router.rule_count(), 1);
        let res = router.resolve("https://api.steampowered.com/ISteamRemoteStorage/");
        assert_eq!(res.url, "https://127.0.0.1:9000/ISteamRemoteStorage/");
        assert_eq!(res.original_host.as_deref(), Some("api.steampowered.com"));
    }

    #[test]
    fn rewrites_ports_cleanly_including_clearing_and_port_80() {
        let mut clear_port = base_rule("cdn.mirror.local");
        clear_port.rules = Some("api.example.com:9000".to_string());

        let mut keep_80 = base_rule("proxy.local:80");
        keep_80.rules = Some("cdn.example.com".to_string());

        let router = MirrorRouter::from_rules(vec![clear_port, keep_80]);
        assert_eq!(
            router.resolve("https://api.example.com:9000/a.vpk").url,
            "https://cdn.mirror.local/a.vpk"
        );
        assert_eq!(
            router.resolve("https://cdn.example.com/b.vpk").url,
            "https://proxy.local:80/b.vpk"
        );
    }
}
