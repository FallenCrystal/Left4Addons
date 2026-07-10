use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use regex::Regex;
use std::sync::OnceLock;
use reqwest::{Client, RequestBuilder};
use std::collections::HashMap;

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
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

pub struct MirrorManager;

impl MirrorManager {
    pub fn resolve(url: &str) -> (String, Option<String>, Option<HashMap<String, String>>) {
        let mut rules = Vec::new();
        if let Some(runtime_dir) = RUNTIME_DIR.get() {
            let mirrors_path = runtime_dir.join("mirrors.json");
            if let Ok(content) = fs::read_to_string(&mirrors_path) {
                if let Ok(parsed_rules) = serde_json::from_str::<Vec<MirrorRule>>(&content) {
                    rules = parsed_rules;
                }
            }
        }
        
        if rules.is_empty() {
            return (url.to_string(), None, None);
        }

        if let Ok(mut parsed_url) = reqwest::Url::parse(url) {
            let host_opt = parsed_url.host_str().map(|s| {
                if let Some(port) = parsed_url.port() {
                    format!("{}:{}", s, port)
                } else {
                    s.to_string()
                }
            });
            if let Some(host) = host_opt {
                for rule in rules {
                    let final_regex_str = if let Some(regex_str) = &rule.regex {
                        regex_str.clone()
                    } else if let Some(domain) = &rule.domain {
                        let mut re = String::from("^");
                        if rule.allow_www {
                            re.push_str(r"(?:www\.)?");
                        }
                        if let Some(subs) = &rule.subdomains {
                            if subs.is_empty() {
                                // No subdomains
                            } else if subs.len() == 1 && subs[0] == "*" {
                                re.push_str(r"(.*)\.");
                            } else {
                                let escaped_subs: Vec<String> = subs.iter().map(|s| regex::escape(s)).collect();
                                re.push_str(&format!("({})\\.", escaped_subs.join("|")));
                            }
                        }
                        re.push_str(&regex::escape(domain));
                        re.push_str("$");
                        re
                    } else if let Some(rules_str) = &rule.rules {
                        // Quick parse of `rules`: e.g. "[store,media].steampowered.com"
                        let mut regex_str = String::new();
                        let mut in_bracket = false;
                        let mut current_group = String::new();
                        let chars: Vec<char> = rules_str.chars().collect();
                        let mut i = 0;

                        while i < chars.len() {
                            let c = chars[i];
                            if c == '[' {
                                in_bracket = true;
                                regex_str.push('(');
                            } else if c == ']' {
                                in_bracket = false;
                                // Escape each item inside the bracket properly
                                let parts: Vec<String> = current_group.split(',')
                                    .map(|s| regex::escape(s))
                                    .collect();
                                regex_str.push_str(&parts.join("|"));
                                current_group.clear();
                                regex_str.push(')');

                                // Check for optional '?'
                                if i + 1 < chars.len() && chars[i + 1] == '?' {
                                    regex_str.push('?');
                                    i += 1;
                                }
                            } else if in_bracket {
                                current_group.push(c);
                            } else if c == '*' {
                                regex_str.push_str("(.*?)");
                            } else {
                                regex_str.push_str(&regex::escape(&c.to_string()));
                            }
                            i += 1;
                        }
                        format!("^{}$", regex_str)
                    } else {
                        continue; // No matching rule defined
                    };
                    
                    if let Ok(re) = Regex::new(&final_regex_str) {
                        if let Some(caps) = re.captures(&host) {
                            let mut new_host = rule.target.clone();
                            
                            // Replace $1, $2, etc.
                            for (i, mat) in caps.iter().enumerate().skip(1) {
                                if let Some(m) = mat {
                                    let placeholder = format!("${}", i);
                                    if new_host.contains(&placeholder) {
                                        new_host = new_host.replace(&placeholder, m.as_str());
                                    }
                                }
                            }
                            
                            // Backward compatibility for '*'
                            if new_host.contains('*') {
                                // Find the first non-empty capture group (if any) or just the first one
                                let matched_group = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                new_host = new_host.replace("*", matched_group);
                            }
                            
                            let original_host = host.to_string();
                            if parsed_url.set_host(Some(&new_host)).is_ok() {
                                let new_url = parsed_url.to_string();
                                return (new_url, if rule.keep_host { Some(original_host) } else { None }, rule.headers.clone());
                            }
                        }
                    }
                }
            }
        }
        
        (url.to_string(), None, None)
    }
}

pub trait MirrorClientExt {
    fn get_mirrored(&self, url: &str) -> RequestBuilder;
    fn post_mirrored(&self, url: &str) -> RequestBuilder;
}

impl MirrorClientExt for Client {
    fn get_mirrored(&self, url: &str) -> RequestBuilder {
        let (resolved_url, original_host, headers) = MirrorManager::resolve(url);
        let mut req = self.get(&resolved_url);
        if let Some(host) = original_host {
            req = req.header(reqwest::header::HOST, host);
        }
        if let Some(hdrs) = headers {
            for (k, v) in hdrs {
                req = req.header(&k, &v);
            }
        }
        req
    }

    fn post_mirrored(&self, url: &str) -> RequestBuilder {
        let (resolved_url, original_host, headers) = MirrorManager::resolve(url);
        let mut req = self.post(&resolved_url);
        if let Some(host) = original_host {
            req = req.header(reqwest::header::HOST, host);
        }
        if let Some(hdrs) = headers {
            for (k, v) in hdrs {
                req = req.header(&k, &v);
            }
        }
        req
    }
}
