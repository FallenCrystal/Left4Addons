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
    pub rules: String,
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
                    // Quick parse of `rules`: e.g. "[store,media].steampowered.com"
                    // We'll replace [a,b] with (a|b) and build a regex
                    let mut regex_str = String::new();
                    let mut in_bracket = false;
                    let mut current_group = String::new();
                    
                    for c in rule.rules.chars() {
                        if c == '[' {
                            in_bracket = true;
                            regex_str.push('(');
                        } else if c == ']' {
                            in_bracket = false;
                            regex_str.push_str(&current_group.replace(",", "|"));
                            current_group.clear();
                            regex_str.push(')');
                        } else if in_bracket {
                            current_group.push(c);
                        } else if c == '*' {
                            regex_str.push_str("(.*?)");
                        } else {
                            regex_str.push_str(&regex::escape(&c.to_string()));
                        }
                    }
                    
                    let final_pattern = format!("^{}$", regex_str);
                    if let Ok(re) = Regex::new(&final_pattern) {
                        if let Some(caps) = re.captures(&host) {
                            let matched_group = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            let new_host = rule.target.replace("*", matched_group);
                            
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
