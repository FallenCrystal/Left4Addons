use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::str::Lines;

pub type KvValue = Value;

/// 解析 Valve KeyValues (`.txt` / `.vdf` / `addoninfo.txt` / `missions/*.txt`)。
///
/// 所有 Key 均规范化为小写 (消除不同作者的大小写差异)。解析时跳过 URL 之外的 `//` 行注释。
pub fn parse_key_values(text: &str) -> KvValue {
    let tokens = tokenize(text.trim_start_matches('\u{feff}'));

    fn parse_object(tokens: &[Token], index: &mut usize) -> Map<String, Value> {
        let mut obj = Map::new();
        while *index < tokens.len() {
            let tok = &tokens[*index];
            if *tok == Token::Close {
                *index += 1;
                return obj;
            }
            if *tok == Token::Open {
                *index += 1;
                continue;
            }

            let Token::Text(key) = tok else {
                unreachable!("braces handled above");
            };
            let key = key.to_ascii_lowercase();
            *index += 1;
            if *index >= tokens.len() {
                break;
            }

            let next_tok = &tokens[*index];
            if *next_tok == Token::Open {
                *index += 1;
                let sub_obj = parse_object(tokens, index);
                obj.insert(key, Value::Object(sub_obj));
            } else if *next_tok == Token::Close {
                obj.insert(key, Value::String(String::new()));
            } else if let Token::Text(value) = next_tok {
                obj.insert(key, Value::String(value.clone()));
                *index += 1;
            }
        }
        obj
    }

    let mut index = 0usize;
    while index < tokens.len() {
        let tok = &tokens[index];
        index += 1;
        if *tok == Token::Open {
            return Value::Object(parse_object(&tokens, &mut index));
        } else if index < tokens.len() && tokens[index] == Token::Open {
            index += 1;
            return Value::Object(parse_object(&tokens, &mut index));
        }
    }

    Value::Object(Map::new())
}

#[derive(PartialEq)]
enum Token {
    Open,
    Close,
    Text(String),
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut lines = text.lines();
    while let Some(mut line) = lines.next() {
        loop {
            line = line.trim_start();
            if line.is_empty() || line.starts_with("//") {
                break;
            }
            let token = match line.as_bytes()[0] {
                b'{' => {
                    line = &line[1..];
                    Token::Open
                }
                b'}' => {
                    line = &line[1..];
                    Token::Close
                }
                b'"' => {
                    line = &line[1..];
                    Token::Text(read_quoted(&mut line, &mut lines))
                }
                _ => {
                    let end = bare_token_end(line);
                    let token = Token::Text(line[..end].to_owned());
                    line = &line[end..];
                    token
                }
            };
            tokens.push(token);
        }
    }
    tokens
}

fn bare_token_end(line: &str) -> usize {
    let end = line
        .find(|c: char| c.is_whitespace() || matches!(c, '{' | '}' | '"'))
        .unwrap_or(line.len());
    let bare = &line[..end];
    if bare
        .split_once("://")
        .is_some_and(|(scheme, _)| is_url_scheme(scheme))
    {
        end
    } else {
        bare.find("//").unwrap_or(end)
    }
}

fn is_url_scheme(scheme: &str) -> bool {
    scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

fn read_quoted<'a>(line: &mut &'a str, lines: &mut Lines<'a>) -> String {
    let mut value = String::new();
    if let Some(rest) = read_quoted_line(line, &mut value, true) {
        *line = rest;
        return value;
    }
    *line = "";
    if let Some(continuations) = closing_line(lines.clone()) {
        for index in 0..=continuations {
            let next = lines.next().expect("lookahead found this continuation");
            value.push('\n');
            if let Some(rest) = read_quoted_line(next, &mut value, index == continuations) {
                *line = rest;
            }
        }
    }
    value
}

// Return the suffix after a closing quote when this line can close the token.
fn read_quoted_line<'a>(line: &'a str, value: &mut String, can_close: bool) -> Option<&'a str> {
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' if can_close => return Some(chars.as_str()),
            '\\' => match chars.next() {
                Some(c @ ('"' | '\\')) => value.push(c),
                Some('n') => value.push('\n'),
                Some('r') => value.push('\r'),
                Some('t') => value.push('\t'),
                other => {
                    value.push('\\');
                    value.extend(other);
                }
            },
            _ => value.push(c),
        }
    }
    None
}

fn closing_line(lines: Lines<'_>) -> Option<usize> {
    for (index, line) in lines.enumerate() {
        let line = line.trim();
        let mut quotes = 0;
        let mut bare_start = 0;
        let mut content_end = line.len();
        let mut chars = line.char_indices();
        while let Some((pos, c)) = chars.next() {
            match c {
                '"' => {
                    quotes += 1;
                    bare_start = pos + 1;
                    // A quote attached to continuation text can be followed by a comment.
                    if quotes == 1
                        && pos > 0
                        && !line[..pos].ends_with(char::is_whitespace)
                        && chars.as_str().trim_start().starts_with("//")
                    {
                        content_end = pos + 1;
                        break;
                    }
                }
                '\\' => {
                    chars.next();
                }
                '/' if quotes % 2 == 0 && chars.as_str().starts_with('/') => {
                    let token = &line[bare_start..];
                    let is_url = bare_token_end(token) > pos - bare_start;
                    if !is_url {
                        content_end = pos;
                        break;
                    }
                    chars.next();
                }
                c if c.is_whitespace() || matches!(c, '{' | '}') => bare_start = pos + c.len_utf8(),
                _ => {}
            }
        }
        let content = line[..content_end].trim_end();
        if matches!(content, "{" | "}") {
            return None;
        }
        if quotes > 0 {
            return (quotes % 2 == 1).then_some(index);
        }
        if let Some((key, value)) = content.split_once(char::is_whitespace) {
            let identifier = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if identifier
                && (key.to_ascii_lowercase().starts_with("addon")
                    || value.trim().parse::<f64>().is_ok())
            {
                return None;
            }
        }
    }
    None
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
    fn recovers_vltava_version_and_preserves_multiline_description() {
        let text = r#""AddonInfo"
{
        addonversion            "1.0.6
        addontitle      "Vltava"
        addonauthor                 "Buyblolop"
        addontagline        "Ready to fly to Prague?"
        addonauthorSteamID      "buyblolop"
        addonDescription        "Survivors fight their way through the city of Prague, to get to a mall on the
outskirts and escape in a helicopter."
        addonURL0               "https://steamcommunity.com/sharedfiles/filedetails/?id=3564279976"

        addonContent_Campaign           1
        addonContent_Map                1
        addonContent_Skin               0
        addonContent_weapon             0
        addonContent_BossInfected       0
        addonContent_CommonInfected     0
        addonContent_Survivor           0
        addonContent_Sound              0
        addonContent_Music              0
        addonContent_Script             0
        addonContent_Prop               0
}"#;
        let parsed = parse_key_values(text);
        assert_eq!(
            parsed,
            serde_json::json!({
                "addonversion": "1.0.6",
                "addontitle": "Vltava",
                "addonauthor": "Buyblolop",
                "addontagline": "Ready to fly to Prague?",
                "addonauthorsteamid": "buyblolop",
                "addondescription": "Survivors fight their way through the city of Prague, to get to a mall on the\noutskirts and escape in a helicopter.",
                "addonurl0": "https://steamcommunity.com/sharedfiles/filedetails/?id=3564279976",
                "addoncontent_campaign": "1",
                "addoncontent_map": "1",
                "addoncontent_skin": "0",
                "addoncontent_weapon": "0",
                "addoncontent_bossinfected": "0",
                "addoncontent_commoninfected": "0",
                "addoncontent_survivor": "0",
                "addoncontent_sound": "0",
                "addoncontent_music": "0",
                "addoncontent_script": "0",
                "addoncontent_prop": "0"
            })
        );
    }

    #[test]
    fn preserves_three_line_descriptions_and_normalizes_crlf() {
        let text = r#""AddonInfo" {
    addonDescription "First \"line\"
middle has // an inline note with "
last line." // comment with an unmatched "
    addontitle "Still parsed"
}"#;
        let parsed = parse_key_values(&format!("\u{feff}{}", text.replace('\n', "\r\n")));
        assert_eq!(
            parsed["addondescription"],
            "First \"line\"\nmiddle has // an inline note with \"\nlast line."
        );
        assert_eq!(parsed["addontitle"], "Still parsed");
    }

    #[test]
    fn recovers_quotes_before_unquoted_entries_braces_and_eof() {
        let text = r#"AddonInfo {
    addonversion "1.0.6

    // comment with "
    addonContent_Campaign 1
    extra "continuous[spaces]
    enabled 0
    addontitle "Vltava
} // comment with "
"#;
        let parsed = parse_key_values(&text.replace("[spaces]", "  "));
        assert_eq!(parsed["addonversion"], "1.0.6");
        assert_eq!(parsed["addoncontent_campaign"], "1");
        assert_eq!(parsed["extra"], "continuous  ");
        assert_eq!(parsed["enabled"], "0");
        assert_eq!(parsed["addontitle"], "Vltava");
        assert_eq!(parse_key_values("AddonInfo {\nname \"last")["name"], "last");
    }

    #[test]
    fn preserves_unquoted_urls_and_skips_adjacent_comments() {
        let text = r#"AddonInfo {
    http http://example.org/path//item
    https https://example.org // comment
    steam steam://run/550
    steamQuoted "steam://run/550"// another "comment"
    plain value// comment
    empty
}"#;
        let parsed = parse_key_values(text);
        assert_eq!(parsed["http"], "http://example.org/path//item");
        assert_eq!(parsed["https"], "https://example.org");
        assert_eq!(parsed["steam"], "steam://run/550");
        assert_eq!(parsed["steamquoted"], "steam://run/550");
        assert_eq!(parsed["plain"], "value");
        assert_eq!(parsed["empty"], "");
    }

    #[test]
    fn preserves_nested_missions_and_escape_sequences() {
        let text = r#"mission { "Modes" { "Coop" { "1" {
            "Map" "c17_m1"
            "DisplayName" "Plaza \"North\"\\Annex\nNext\rLeft\tEnd\q"
        } } } }"#;
        let parsed = parse_key_values(text);
        let map = &parsed["modes"]["coop"]["1"];
        assert_eq!(map["map"], "c17_m1");
        assert_eq!(
            map["displayname"],
            "Plaza \"North\"\\Annex\nNext\rLeft\tEnd\\q"
        );
        assert!(flatten_kv_strings(&parsed).is_empty());
    }

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
