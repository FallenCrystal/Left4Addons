use regex::Regex;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

pub static RUNTIME_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub struct MirrorManager;

impl MirrorManager {
    pub fn client_builder_for(url: &str) -> reqwest::ClientBuilder {
        let rules = load_rules();
        let insecure_tls = matching_rule(url, &rules).is_some_and(|rule| rule.insecure_tls);

        reqwest::Client::builder().danger_accept_invalid_certs(insecure_tls)
    }

    pub fn resolve(url: &str) -> (String, Option<String>, Option<HashMap<String, String>>) {
        let rules = load_rules();

        resolve_with_rules(url, &rules)
    }
}

fn load_rules() -> Vec<MirrorRule> {
    RUNTIME_DIR
        .get()
        .and_then(|runtime_dir| fs::read_to_string(runtime_dir.join("mirrors.json")).ok())
        .and_then(|content| serde_json::from_str::<Vec<MirrorRule>>(&content).ok())
        .unwrap_or_default()
}

fn matching_rule<'a>(url: &str, rules: &'a [MirrorRule]) -> Option<&'a MirrorRule> {
    let parsed_url = reqwest::Url::parse(url).ok()?;
    let host = parsed_url
        .host_str()
        .map(|hostname| match parsed_url.port() {
            Some(port) => format!("{hostname}:{port}"),
            None => hostname.to_string(),
        })?;

    rules.iter().find(|rule| {
        rule_pattern(rule)
            .and_then(|pattern| Regex::new(&pattern).ok())
            .is_some_and(|regex| regex.is_match(&host))
    })
}

fn resolve_with_rules(
    url: &str,
    rules: &[MirrorRule],
) -> (String, Option<String>, Option<HashMap<String, String>>) {
    if rules.is_empty() {
        return (url.to_string(), None, None);
    }

    let Ok(mut parsed_url) = reqwest::Url::parse(url) else {
        return (url.to_string(), None, None);
    };
    let Some(host) = parsed_url
        .host_str()
        .map(|hostname| match parsed_url.port() {
            Some(port) => format!("{hostname}:{port}"),
            None => hostname.to_string(),
        })
    else {
        return (url.to_string(), None, None);
    };

    for rule in rules {
        let Some(pattern) = rule_pattern(rule) else {
            continue;
        };
        let Ok(regex) = Regex::new(&pattern) else {
            continue;
        };
        let Some(captures) = regex.captures(&host) else {
            continue;
        };

        let target = replace_captures(&rule.target, &captures);
        if set_target_host(&mut parsed_url, &target) {
            return (
                parsed_url.to_string(),
                rule.keep_host.then(|| host.clone()),
                rule.headers.clone(),
            );
        }
    }

    (url.to_string(), None, None)
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

    rule.rules.as_deref().map(string_rule_pattern)
}

fn string_rule_pattern(rule: &str) -> String {
    let patterns = rule
        .split(';')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .map(string_rule_pattern_fragment)
        .collect::<Vec<_>>();

    match patterns.as_slice() {
        [] => "(?!)".to_string(),
        [pattern] => format!("^{pattern}$"),
        _ => format!("^(?:{})$", patterns.join("|")),
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

fn set_target_host(url: &mut reqwest::Url, target: &str) -> bool {
    let Ok(target_url) = reqwest::Url::parse(&format!("http://{target}")) else {
        return false;
    };
    let Some(target_host) = target_url.host_str() else {
        return false;
    };
    if url.set_host(Some(target_host)).is_err() {
        return false;
    }
    if let Some(port) = target_url.port() {
        if url.set_port(Some(port)).is_err() {
            return false;
        }
    }
    true
}

pub trait MirrorClientExt {
    fn get_mirrored(&self, url: &str) -> RequestBuilder;
    fn post_mirrored(&self, url: &str) -> RequestBuilder;
}

impl MirrorClientExt for Client {
    fn get_mirrored(&self, url: &str) -> RequestBuilder {
        let (resolved_url, original_host, headers) = MirrorManager::resolve(url);
        mirrored_request(self.get(resolved_url), original_host, headers)
    }

    fn post_mirrored(&self, url: &str) -> RequestBuilder {
        let (resolved_url, original_host, headers) = MirrorManager::resolve(url);
        mirrored_request(self.post(resolved_url), original_host, headers)
    }
}

fn mirrored_request(
    request: RequestBuilder,
    original_host: Option<String>,
    headers: Option<HashMap<String, String>>,
) -> RequestBuilder {
    let mut request = request;
    if let Some(host) = original_host {
        request = request.header(reqwest::header::HOST, host);
    }
    if let Some(headers) = headers {
        for (key, value) in headers {
            request = request.header(key, value);
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(target: &str) -> MirrorRule {
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
    fn object_rule_replaces_subdomain_and_preserves_headers() {
        let mut rule = rule("$1.mirror.example.com");
        rule.domain = Some("steamstatic.com".to_string());
        rule.subdomains = Some(vec!["store".to_string(), "media".to_string()]);
        rule.allow_www = true;
        rule.keep_host = true;
        rule.headers = Some(HashMap::from([(
            "X-Mirror".to_string(),
            "enabled".to_string(),
        )]));

        let (url, host, headers) =
            resolve_with_rules("https://www.media.steamstatic.com/path?q=1", &[rule]);

        assert_eq!(url, "https://media.mirror.example.com/path?q=1");
        assert_eq!(host.as_deref(), Some("www.media.steamstatic.com"));
        assert_eq!(
            headers.unwrap().get("X-Mirror").map(String::as_str),
            Some("enabled")
        );
    }

    #[test]
    fn string_rule_uses_capture_groups_and_target_port() {
        let mut rule = rule("$2.proxy.example.com:8443");
        rule.rules = Some("[www.]?*.steamcommunity.com".to_string());

        let (url, host, _) = resolve_with_rules(
            "https://www.workshop.steamcommunity.com/files?id=42",
            &[rule],
        );

        assert_eq!(url, "https://workshop.proxy.example.com:8443/files?id=42");
        assert_eq!(host, None);
    }

    #[test]
    fn invalid_or_unmatched_rules_leave_url_unchanged() {
        let mut invalid = rule("mirror.example.com");
        invalid.regex = Some("[".to_string());
        let source = "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/";

        assert_eq!(resolve_with_rules(source, &[invalid]).0, source);
        assert_eq!(resolve_with_rules(source, &[]).0, source);
    }

    #[test]
    fn string_rule_supports_semicolon_separated_hosts() {
        let mut rule = rule("mirror.example.com");
        rule.rules = Some("cdn.steamstatic.com; community.steamstatic.com".to_string());

        assert_eq!(
            resolve_with_rules("https://community.steamstatic.com/public", &[rule]).0,
            "https://mirror.example.com/public"
        );
    }
}
