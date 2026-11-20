use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub type KvValue = Value;

/// 解析 Valve KeyValues (`.txt` / `.vdf` / `addoninfo.txt` / `missions/*.txt`)。
///
/// 所有 Key 均规范化为小写 (消除不同作者的大小写差异)。解析时跳过 URL 之外的 `//` 行注释。
pub fn parse_key_values(text: &str) -> KvValue {
    let mut clean_text = String::new();
    for line in text.lines() {
        let mut clean_line = String::new();
        let mut chars = line.chars().peekable();
        let mut in_quote = false;
        let mut escaped = false;

        while let Some(c) = chars.next() {
            if escaped {
                clean_line.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' {
                clean_line.push(c);
                escaped = true;
                continue;
            }
            if c == '"' {
                in_quote = !in_quote;
                clean_line.push(c);
                continue;
            }
            if !in_quote && c == '/' && chars.peek() == Some(&'/') {
                let preceded = clean_line.ends_with(':') || clean_line.ends_with('"');
                if !preceded {
                    break;
                }
            }
            clean_line.push(c);
        }
        clean_text.push_str(&clean_line);
        clean_text.push('\n');
    }

    let mut tokens = Vec::new();
    let mut chars = clean_text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '{' || c == '}' {
            tokens.push(c.to_string());
            chars.next();
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            s.push(chars.next().unwrap());
            let mut escaped = false;
            for nc in chars.by_ref() {
                s.push(nc);
                if escaped {
                    escaped = false;
                } else if nc == '\\' {
                    escaped = true;
                } else if nc == '"' {
                    break;
                }
            }
            tokens.push(s);
        } else {
            let mut s = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_whitespace() || nc == '{' || nc == '}' || nc == '"' {
                    break;
                }
                s.push(chars.next().unwrap());
            }
            if !s.is_empty() {
                tokens.push(s);
            }
        }
    }

    fn clean_token(t: &str) -> String {
        if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
            let inner = &t[1..t.len() - 1];
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some('t') => out.push('\t'),
                        Some(other) => {
                            out.push('\\');
                            out.push(other);
                        }
                        None => out.push('\\'),
                    }
                } else {
                    out.push(c);
                }
            }
            out
        } else {
            t.to_string()
        }
    }

    fn parse_object(tokens: &[String], index: &mut usize) -> Map<String, Value> {
        let mut obj = Map::new();
        while *index < tokens.len() {
            let tok = &tokens[*index];
            if tok == "}" {
                *index += 1;
                return obj;
            }
            if tok == "{" {
                *index += 1;
                continue;
            }

            let key = clean_token(tok).to_ascii_lowercase();
            *index += 1;
            if *index >= tokens.len() {
                break;
            }

            let next_tok = &tokens[*index];
            if next_tok == "{" {
                *index += 1;
                let sub_obj = parse_object(tokens, index);
                obj.insert(key, Value::Object(sub_obj));
            } else if next_tok == "}" {
                obj.insert(key, Value::String(String::new()));
            } else {
                let val = clean_token(next_tok);
                obj.insert(key, Value::String(val));
                *index += 1;
            }
        }
        obj
    }

    let mut index = 0usize;
    while index < tokens.len() {
        let tok = &tokens[index];
        index += 1;
        if tok == "{" {
            return Value::Object(parse_object(&tokens, &mut index));
        } else if index < tokens.len() && tokens[index] == "{" {
            index += 1;
            return Value::Object(parse_object(&tokens, &mut index));
        }
    }

    Value::Object(Map::new())
}

pub fn flatten_kv_strings(val: &Value) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(obj) = val.as_object() {
        for (k, v) in obj {
            match v {
                Value::String(s) => {
                    map.insert(k.clone(), s.clone());
                }
                Value::Number(n) => {
                    map.insert(k.clone(), n.to_string());
                }
                Value::Bool(b) => {
                    map.insert(k.clone(), b.to_string());
                }
                _ => {}
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addoninfo_preserving_urls_and_lowercasing_keys() {
        let input = r#"
// file header comment
"AddonInfo"
{
    "addonTitle" "Cold Stream Remix"
    "addonURL0" "https://steamcommunity.com/sharedfiles/filedetails/?id=123456"
    "addonContent_Campaign" 1 // trailing comment
}
"#;
        let parsed = parse_key_values(input);
        assert_eq!(parsed["addontitle"], "Cold Stream Remix");
        assert_eq!(
            parsed["addonurl0"],
            "https://steamcommunity.com/sharedfiles/filedetails/?id=123456"
        );
        assert_eq!(parsed["addoncontent_campaign"], "1");
    }
}
