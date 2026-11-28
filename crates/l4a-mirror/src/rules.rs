use crate::{MirrorError, MirrorRouter};
use regex::Regex;
use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

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
pub(crate) struct CompiledRule {
    pub(crate) regex: Regex,
    pub(crate) raw: MirrorRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub url: String,
    pub original_host: Option<String>,
    pub headers: HashMap<String, String>,
    pub insecure_tls: bool,
    /// A configured rule rewrote the authority or supplied request headers.
    pub matched_rule: bool,
}

impl ResolvedRequest {
    pub(crate) fn has_mirror_overrides(&self, effective_insecure_tls: bool) -> bool {
        effective_insecure_tls || self.original_host.is_some() || !self.headers.is_empty()
    }

    pub(crate) fn rewrites_authority(&self, source: &url::Url) -> bool {
        self.matched_rule
            && url::Url::parse(&self.url)
                .is_ok_and(|target| url_authority(&target) != url_authority(source))
    }
}

impl MirrorRouter {
    pub fn from_rules(rules: Vec<MirrorRule>) -> Self {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let Some(pattern) = rule_pattern(&rule) else {
                continue;
            };
            match Regex::new(&pattern) {
                Ok(regex) => compiled.push(CompiledRule { regex, raw: rule }),
                Err(err) => {
                    tracing::warn!(pattern = %pattern, error = %err, "Ignoring invalid mirror rule regex");
                }
            }
        }
        let rule_fingerprint = if compiled.is_empty() {
            [0; 32]
        } else {
            let mut hasher = Sha256::new();
            for rule in &compiled {
                let raw = &rule.raw;
                let headers = raw
                    .headers
                    .as_ref()
                    .map(|headers| headers.iter().collect::<BTreeMap<_, _>>());
                let encoded = serde_json::to_vec(&(
                    rule.regex.as_str(),
                    &raw.target,
                    raw.keep_host,
                    raw.insecure_tls,
                    headers,
                ))
                .expect("mirror rules are serializable");
                hasher.update(encoded);
            }
            hasher.finalize().into()
        };
        Self {
            rules: compiled,
            rule_fingerprint,
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

    /// Stable identifier for the effective, ordered rules of this router.
    pub fn rule_fingerprint(&self) -> &[u8; 32] {
        &self.rule_fingerprint
    }

    pub fn resolve(&self, url: &str) -> ResolvedRequest {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.resolve_with_https_only(url, https_only)
    }

    pub fn resolve_with_https_only(&self, url: &str, https_only: bool) -> ResolvedRequest {
        Self::resolve_with_rules(url, https_only, &self.rules)
    }

    pub(crate) fn resolve_with_rules(
        url: &str,
        https_only: bool,
        rules: &[CompiledRule],
    ) -> ResolvedRequest {
        let direct = |url: String| ResolvedRequest {
            url,
            original_host: None,
            headers: HashMap::new(),
            insecure_tls: false,
            matched_rule: false,
        };

        let Ok(mut parsed_url) = url::Url::parse(url) else {
            return direct(url.to_string());
        };

        let Some(host) = parsed_url
            .host_str()
            .map(|hostname| match parsed_url.port() {
                Some(port) => format!("{hostname}:{port}"),
                None => hostname.to_string(),
            })
        else {
            return direct(url.to_string());
        };

        let upgrade_cdn = is_legacy_steam_cdn_url(&parsed_url);
        for compiled in rules {
            let Some(captures) = compiled.regex.captures(&host) else {
                continue;
            };
            let target = replace_captures(&compiled.raw.target, &captures);
            let mut routed_url = parsed_url.clone();
            // 允许 HTTP 时镜像沿用原始协议。仅 HTTPS 模式先升级，再应用镜像端口。
            if upgrade_cdn && https_only {
                let _ = routed_url.set_scheme("https");
            }
            if set_target_host(&mut routed_url, &target) {
                let original_host = compiled.raw.keep_host.then_some(host);
                let headers = compiled.raw.headers.clone().unwrap_or_default();
                let matched_rule = url_authority(&routed_url) != url_authority(&parsed_url)
                    || original_host.is_some()
                    || !headers.is_empty();
                return ResolvedRequest {
                    url: routed_url.into(),
                    original_host,
                    headers,
                    insecure_tls: compiled.raw.insecure_tls,
                    matched_rule,
                };
            }
        }

        if upgrade_cdn && parsed_url.set_scheme("https").is_ok() {
            direct(parsed_url.into())
        } else {
            direct(url.to_string())
        }
    }

    pub fn validate_url(&self, url: &str) -> Result<ResolvedRequest, MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.validate_url_with_policy(url, https_only)
    }

    pub fn validate_url_with_https_only(
        &self,
        url: &str,
        https_only: bool,
    ) -> Result<ResolvedRequest, MirrorError> {
        self.validate_url_with_policy(url, https_only)
    }

    pub(crate) fn validate_url_with_policy(
        &self,
        url: &str,
        https_only: bool,
    ) -> Result<ResolvedRequest, MirrorError> {
        let resolved = self.resolve_with_https_only(url, https_only);
        ensure_https_if_required(&resolved.url, https_only)?;
        Ok(resolved)
    }
}

pub(crate) fn ensure_https_if_required(url: &str, https_only: bool) -> Result<(), MirrorError> {
    if https_only && !url::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https") {
        return Err(MirrorError::HttpsRequired);
    }
    Ok(())
}

pub(crate) fn url_authority(url: &url::Url) -> (Option<&str>, Option<u16>) {
    (url.host_str(), url.port())
}

pub(crate) fn apply_resolved_headers(
    mut req: RequestBuilder,
    resolved: &ResolvedRequest,
) -> RequestBuilder {
    if let Some(ref host) = resolved.original_host {
        req = req.header(reqwest::header::HOST, host);
    }
    for (k, v) in &resolved.headers {
        req = req.header(k, v);
    }
    req
}

fn is_legacy_steam_cdn_url(url: &url::Url) -> bool {
    url.scheme() == "http"
        && url.port().is_none()
        && url.host_str().is_some_and(|host| {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            let is_cloud_host = host.strip_suffix(".steampowered.com").is_some_and(|sub| {
                !sub.contains('.')
                    && (sub == "cloud"
                        || sub.starts_with("cloud-")
                        || sub.strip_prefix("cloud").is_some_and(|digits| {
                            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                        }))
            });
            host == "steamusercontent.com"
                || host.ends_with(".steamusercontent.com")
                || host == "steamstatic.com"
                || host.ends_with(".steamstatic.com")
                || host == "media.steampowered.com"
                || (is_cloud_host && url.path().starts_with("/ugc/"))
                || host.strip_suffix(".akamaihd.net").is_some_and(|prefix| {
                    !prefix.contains('.')
                        && (matches!(prefix, "steamuserimages" | "steamcdn")
                            || prefix.starts_with("steamuserimages-")
                            || prefix.starts_with("steamcdn-"))
                })
        })
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
