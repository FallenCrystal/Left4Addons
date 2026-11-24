use super::{Token, MAX_COMPACT_LEN};
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct TokenizedText {
    pub(super) tokens: Vec<Token>,
    // The original word and the position of its first split token.
    pub(super) unsplit_words: Vec<(usize, String)>,
}

pub fn tokenize_text(raw: &str) -> Vec<Token> {
    tokenize_with_aliases(raw).tokens
}

pub(super) fn tokenize_with_aliases(raw: &str) -> TokenizedText {
    let mut text = TokenizedText::default();
    let mut word_buf = String::new();

    let flush_word = |buf: &mut String, out: &mut TokenizedText| {
        if buf.is_empty() {
            return;
        }
        let parts = split_word_run(buf);
        if parts.len() > 1 {
            out.unsplit_words.push((out.tokens.len(), parts.concat()));
        }
        out.tokens.extend(parts.into_iter().map(Token::Word));
        buf.clear();
    };

    for ch in raw.chars() {
        let ch = fold_char(ch);
        if is_cjk_split_char(ch) {
            flush_word(&mut word_buf, &mut text);
            text.tokens.push(Token::Cjk(ch));
        } else if ch.is_alphanumeric() {
            word_buf.push(ch);
        } else {
            flush_word(&mut word_buf, &mut text);
        }
    }
    flush_word(&mut word_buf, &mut text);
    text
}

/// Splits an alphanumeric run on CamelCase transitions (e.g. `DayBreak` -> `["day", "break"]`,
/// `YiChangCity2` -> `["yi", "chang", "city", "2"]`, `HTMLParser` -> `["html", "parser"]`) while
/// preserving single-letter suffixes like `MissionX` and compact codes like `L4D2`, `M60`, `v2`.
pub(super) fn split_word_run(run: &str) -> Vec<String> {
    let mut chars = run.char_indices().peekable();
    let Some((_, mut prev)) = chars.next() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut push_part = |part: &str| {
        let lowered = fold_text(part);
        // Split trailing digits off long words (e.g. "city2" -> "city", "2"; "part1" -> "part", "1";
        // "cyberpunk2077" -> "cyberpunk", "2077"), while keeping codes like "l4d2", "m60", "v2", "lv8".
        if let Some((prefix, digits)) = split_long_word_trailing_digits(&lowered) {
            out.push(prefix.to_string());
            out.push(digits.to_string());
        } else if !lowered.is_empty() {
            out.push(lowered);
        }
    };
    let mut start = 0;
    while let Some((pos, curr)) = chars.next() {
        if curr.is_uppercase()
            && chars.peek().is_some_and(|(_, next)| next.is_lowercase())
            && (prev.is_lowercase() || prev.is_uppercase() || prev.is_ascii_digit())
        {
            push_part(&run[start..pos]);
            start = pos;
        }
        prev = curr;
    }
    push_part(&run[start..]);
    out
}

fn split_long_word_trailing_digits(word: &str) -> Option<(&str, &str)> {
    let digit_start = word.find(|c: char| c.is_ascii_digit())?;
    let prefix = &word[..digit_start];
    let suffix = &word[digit_start..];
    if prefix.chars().count() >= 3
        && prefix.chars().all(char::is_alphabetic)
        && !suffix.is_empty()
        && suffix.chars().all(|c| c.is_ascii_digit())
    {
        Some((prefix, suffix))
    } else {
        None
    }
}

pub(super) fn build_compact_tokens(text: &TokenizedText, max_span: usize) -> String {
    let mut emitted = HashSet::new();
    let mut out = Vec::new();
    let mut buf = String::new();

    let tokens = &text.tokens;
    for start in 0..tokens.len() {
        if tokens[start].as_word().is_none() {
            continue;
        }
        buf.clear();
        let max_end = (start + max_span).min(tokens.len());
        for (offset, tok) in tokens[start..max_end].iter().enumerate() {
            let Some(w) = tok.as_word() else {
                break;
            };
            if buf.len() + w.len() > MAX_COMPACT_LEN {
                break;
            }
            buf.push_str(w);
            if offset >= 1 && emitted.insert(buf.clone()) {
                out.push(buf.clone());
            }
        }
    }

    for (_, word) in &text.unsplit_words {
        if emitted.insert(word.clone()) {
            out.push(word.clone());
        }
    }

    out.join(" ")
}

pub(super) fn tokens_to_fts_string(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        match tok {
            Token::Word(w) => out.push_str(w),
            Token::Cjk(c) => out.push(*c),
        }
    }
    out
}

pub fn strip_bbcode_and_urls(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    let mut missing_closers = HashSet::new();

    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close_rel) = raw[i..].find(']') {
                let tag_inner = &raw[i + 1..i + close_rel];
                let trimmed_tag = tag_inner.trim();
                let lower_name = trimmed_tag
                    .strip_prefix('/')
                    .unwrap_or(trimmed_tag)
                    .split(['=', ' '])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();

                if is_bbcode_tag(&lower_name) {
                    i += close_rel + 1;
                    if !trimmed_tag.starts_with('/') {
                        let close_tag = match lower_name.as_str() {
                            "img" => Some("[/img]"),
                            "url" if !trimmed_tag.contains('=') => Some("[/url]"),
                            "previewyoutube" => Some("[/previewyoutube]"),
                            _ => None,
                        };
                        if let Some(tag) = close_tag.filter(|tag| !missing_closers.contains(tag)) {
                            if let Some(pos) = find_ascii_case_insensitive(&raw[i..], tag) {
                                i += pos + tag.len();
                            } else {
                                missing_closers.insert(tag);
                            }
                        }
                    }
                    out.push(' ');
                    continue;
                }
            }
        }

        if matches!(bytes[i], b'h' | b'H' | b's' | b'S')
            && ["http://", "https://", "steam://"].iter().any(|scheme| {
                bytes[i..]
                    .get(..scheme.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme.as_bytes()))
            })
        {
            let rest = &raw[i..];
            i += rest
                .find(|ch: char| ch.is_whitespace() || matches!(ch, ']' | ')' | '>' | '"' | '\''))
                .unwrap_or(rest.len());
            out.push(' ');
            continue;
        }

        let ch = raw[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn find_ascii_case_insensitive(haystack: &str, needle_lower: &str) -> Option<usize> {
    let n = needle_lower.len();
    if n == 0 || haystack.len() < n {
        return None;
    }
    let h = haystack.as_bytes();
    let nl = needle_lower.as_bytes();
    for pos in 0..=(h.len() - n) {
        if h[pos..pos + n]
            .iter()
            .zip(nl.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
        {
            return Some(pos);
        }
    }
    None
}

fn is_bbcode_tag(name: &str) -> bool {
    matches!(
        name,
        "b" | "i"
            | "u"
            | "strike"
            | "s"
            | "h1"
            | "h2"
            | "h3"
            | "list"
            | "olist"
            | "*"
            | "url"
            | "img"
            | "quote"
            | "code"
            | "noparse"
            | "spoiler"
            | "table"
            | "tr"
            | "td"
            | "th"
            | "hr"
            | "previewyoutube"
            | "dynamiclink"
    )
}

pub fn fold_text(raw: &str) -> String {
    if raw.is_ascii() {
        return raw.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let ch = fold_char(ch);
        for lc in ch.to_lowercase() {
            out.push(fold_latin_diacritic(lc));
        }
    }
    out
}

pub(super) fn fold_char(ch: char) -> char {
    match ch {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch),
        '’' | '‘' => '\'',
        '“' | '”' => '"',
        '–' | '—' => '-',
        _ => ch,
    }
}

fn fold_latin_diacritic(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ł' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ŕ' | 'ř' => 'r',
        'ś' | 'š' | 'ş' => 's',
        'ť' | 'ţ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => ch,
    }
}

pub(super) fn is_han(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF
    )
}

fn is_kana(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF)
}

pub(super) fn is_cjk_split_char(ch: char) -> bool {
    is_han(ch) || is_kana(ch)
}

pub(super) fn is_pure_ascii_alpha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())
}
