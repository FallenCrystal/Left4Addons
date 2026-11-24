use super::query::fts_quote;
use super::text::{fold_char, is_han, is_pure_ascii_alpha};
use super::{QueryTerm, MAX_PINYIN_SPAN, MIN_PINYIN_LEN, PINYIN_PAIR_KEY_LEN};
use pinyin::ToPinyinMulti;
use std::collections::HashSet;

#[derive(Debug)]
pub(super) struct HanPinyinSpan {
    starts_title: bool,
    readings: Vec<Vec<String>>,
}

pub(super) fn collect_han_pinyin_spans(title: &str) -> Vec<HanPinyinSpan> {
    let mut spans = Vec::new();
    let mut seen_any_token = false;
    let mut run_start_is_title_start = false;
    let mut current_run: Vec<Vec<String>> = Vec::new();

    for ch in title.chars() {
        let ch = fold_char(ch);
        if is_han(ch) {
            if current_run.is_empty() {
                run_start_is_title_start = !seen_any_token;
            }
            let r = han_pinyin_readings(ch);
            if !r.is_empty() {
                current_run.push(r);
            } else {
                if current_run.len() >= 2 {
                    spans.push(HanPinyinSpan {
                        starts_title: run_start_is_title_start,
                        readings: std::mem::take(&mut current_run),
                    });
                } else {
                    current_run.clear();
                }
            }
            seen_any_token = true;
        } else {
            if current_run.len() >= 2 {
                spans.push(HanPinyinSpan {
                    starts_title: run_start_is_title_start,
                    readings: std::mem::take(&mut current_run),
                });
            } else {
                current_run.clear();
            }
            if ch.is_alphanumeric() {
                seen_any_token = true;
            }
        }
    }

    if current_run.len() >= 2 {
        spans.push(HanPinyinSpan {
            starts_title: run_start_is_title_start,
            readings: current_run,
        });
    }

    spans
}

pub(super) fn match_term_in_pinyin(term: &QueryTerm, spans: &[HanPinyinSpan]) -> Option<bool> {
    match term {
        QueryTerm::Word {
            compact,
            exact_only: false,
            ..
        } if is_pure_ascii_alpha(compact) && compact.len() >= MIN_PINYIN_LEN => {
            match_pinyin_string(compact, spans)
        }
        _ => None,
    }
}

/// Returns `Some(true)` if `needle` matches a >= 2 Han char pinyin sequence starting at the very
/// start of the title, `Some(false)` if it matches elsewhere inside a Han run, or `None`.
pub(super) fn match_pinyin_string(needle: &str, spans: &[HanPinyinSpan]) -> Option<bool> {
    let mut found_anywhere = false;
    for span in spans {
        for start in 0..span.readings.len() {
            if matches_pinyin_from(needle, &span.readings[start..], 0) {
                if span.starts_title && start == 0 {
                    return Some(true);
                }
                found_anywhere = true;
            }
        }
    }
    found_anywhere.then_some(false)
}

fn matches_pinyin_from(rest: &str, readings: &[Vec<String>], matched_chars: usize) -> bool {
    if rest.is_empty() {
        return matched_chars >= 2;
    }
    if readings.is_empty() || matched_chars >= MAX_PINYIN_SPAN {
        return false;
    }
    for syllable in &readings[0] {
        if let Some(next_rest) = rest.strip_prefix(syllable.as_str()) {
            if matches_pinyin_from(next_rest, &readings[1..], matched_chars + 1) {
                return true;
            }
        }
    }
    false
}

pub(super) fn build_pinyin_tokens(title: &str) -> String {
    if title.is_ascii() {
        return String::new();
    }

    let mut emitted = HashSet::new();
    let mut out = Vec::new();
    for span in collect_han_pinyin_spans(title) {
        for pair in span.readings.windows(2) {
            for first in &pair[0] {
                for second in &pair[1] {
                    let mut key = format!("{first}{second}");
                    key.truncate(key.len().min(PINYIN_PAIR_KEY_LEN));
                    if emitted.insert(key.clone()) {
                        out.push(key);
                    }
                }
            }
        }
    }
    out.join(" ")
}

pub(super) fn pinyin_fts_clause(needle: &str) -> Option<String> {
    if needle.len() < MIN_PINYIN_LEN || !is_pure_ascii_alpha(needle) {
        return None;
    }

    // Every reading the scorer accepts starts with an adjacent pair. Probe all possible
    // lengths of that pair's key and let the scorer verify the full reading in order.
    let keys = (2..=needle.len().min(PINYIN_PAIR_KEY_LEN))
        .map(|len| fts_quote(&needle[..len]))
        .collect::<Vec<_>>()
        .join(" OR ");
    Some(format!("title_pinyin : ({keys})"))
}

fn han_pinyin_readings(ch: char) -> Vec<String> {
    let Some(multi) = ch.to_pinyin_multi() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for py in multi {
        let plain = py.plain().to_ascii_lowercase();
        if !plain.is_empty()
            && plain.bytes().all(|b| b.is_ascii_lowercase())
            && !out.contains(&plain)
        {
            out.push(plain);
        }
        // Emit `v` variant for `ü` syllables (e.g. 女 -> nv, 绿 -> lv) so Chinese IME spelling matches.
        let with_tone = py.with_tone();
        if with_tone
            .chars()
            .any(|c| matches!(c, 'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ'))
        {
            let v_variant: String = with_tone
                .chars()
                .filter_map(|c| match c {
                    'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => Some('v'),
                    'ā' | 'á' | 'ǎ' | 'à' => Some('a'),
                    'ē' | 'é' | 'ě' | 'è' => Some('e'),
                    'ī' | 'í' | 'ǐ' | 'ì' => Some('i'),
                    'ō' | 'ó' | 'ǒ' | 'ò' => Some('o'),
                    'ū' | 'ú' | 'ǔ' | 'ù' => Some('u'),
                    c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                    _ => None,
                })
                .collect();
            if !v_variant.is_empty() && !out.contains(&v_variant) {
                out.push(v_variant);
            }
        }
    }
    out
}
