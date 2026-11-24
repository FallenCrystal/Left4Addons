use super::pinyin::pinyin_fts_clause;
use super::text::{
    fold_char, fold_text, is_cjk_split_char, is_pure_ascii_alpha, split_word_run, tokenize_text,
};
use super::{
    ParsedQuery, QueryTerm, Token, MAX_COMPACT_LEN, MAX_COMPACT_SPAN, MAX_PINYIN_SPAN,
    MIN_COMPACT_PREFIX_LEN, MIN_CREATOR_PREFIX_LEN, MIN_PREFIX_LEN,
};
use std::collections::HashSet;
use url::Url;

pub fn parse_query(raw: &str) -> Option<ParsedQuery> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let raw_folded = fold_text(trimmed);

    // Direct Steam Workshop URL lookup
    if let Some(wid) = extract_steam_url_id(trimmed) {
        return Some(ParsedQuery {
            raw_trimmed: trimmed.to_string(),
            raw_folded,
            exact_workshop_id: Some(wid),
            numeric_prefix: None,
            terms: Vec::new(),
            whole_compact: None,
            literal_substrings: Vec::new(),
            compact_all: String::new(),
        });
    }

    // Pure numeric Workshop ID (with optional leading `#`)
    let numeric_candidate = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let (exact_workshop_id, numeric_prefix) =
        if !numeric_candidate.is_empty() && numeric_candidate.chars().all(|c| c.is_ascii_digit()) {
            let exact = numeric_candidate.parse::<u64>().ok().filter(|id| *id > 0);
            (exact, Some(numeric_candidate.to_string()))
        } else {
            (None, None)
        };

    // Detect explicit leading/trailing `_` constraints (e.g. `mission_`) so they stay exact.
    let mut literal_substrings = Vec::new();
    let mut exact_only_words = HashSet::new();
    for raw_chunk in trimmed.split_whitespace() {
        let folded_chunk = fold_text(raw_chunk);
        if (folded_chunk.starts_with('_') || folded_chunk.ends_with('_'))
            && folded_chunk.chars().any(char::is_alphanumeric)
        {
            literal_substrings.push(folded_chunk.clone());
            for tok in tokenize_text(raw_chunk) {
                if let Token::Word(w) = tok {
                    exact_only_words.insert(w);
                }
            }
        }
    }

    let terms = parse_query_terms(trimmed, &exact_only_words);
    let compact_all = query_terms_compact_all(&terms);

    let all_words = !terms.is_empty() && terms.iter().all(|t| matches!(t, QueryTerm::Word { .. }));
    let any_exact_literal = !literal_substrings.is_empty();
    let whole_compact = if all_words && !any_exact_literal && terms.len() >= 2 {
        let all_ascii_alpha = terms.iter().all(|t| match t {
            QueryTerm::Word { compact, .. } => is_pure_ascii_alpha(compact),
            QueryTerm::Cjk(_) => false,
        });
        let max_span = if all_ascii_alpha {
            MAX_PINYIN_SPAN
        } else {
            MAX_COMPACT_SPAN
        };
        if terms.len() <= max_span {
            if compact_all.len() <= MAX_COMPACT_LEN {
                Some(compact_all.clone())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Some(ParsedQuery {
        raw_trimmed: trimmed.to_string(),
        raw_folded,
        exact_workshop_id,
        numeric_prefix,
        terms,
        whole_compact,
        literal_substrings,
        compact_all,
    })
}

pub fn build_primary_fts_query(query: &ParsedQuery) -> Option<String> {
    build_primary_fts_query_inner(query, true)
}

pub fn build_title_only_fts_query(query: &ParsedQuery) -> Option<String> {
    build_primary_fts_query_inner(query, false)
}

fn build_primary_fts_query_inner(query: &ParsedQuery, include_creator: bool) -> Option<String> {
    if query.terms.is_empty() {
        return None;
    }

    let mut term_clauses = Vec::with_capacity(query.terms.len());
    for term in &query.terms {
        match term {
            QueryTerm::Cjk(chars) => {
                let phrase = cjk_phrase(chars);
                if include_creator {
                    term_clauses.push(format!("{{title_tokens creator_tokens}} : {phrase}"));
                } else {
                    term_clauses.push(format!("title_tokens : {phrase}"));
                }
            }
            QueryTerm::Word {
                compact,
                sub_tokens,
                exact_only,
            } => {
                let mut branches = Vec::new();
                let char_len = compact.chars().count();
                let q = fts_quote(compact);
                if *exact_only || char_len < MIN_PREFIX_LEN {
                    if include_creator {
                        branches.push(format!(
                            "{{title_tokens title_compact creator_tokens}} : {q}"
                        ));
                    } else {
                        branches.push(format!("{{title_tokens title_compact}} : {q}"));
                    }
                } else {
                    branches.push(format!("title_tokens : {q}*"));
                    if char_len >= MIN_COMPACT_PREFIX_LEN {
                        branches.push(format!("title_compact : {q}*"));
                    } else {
                        branches.push(format!("title_compact : {q}"));
                    }
                    if include_creator {
                        if char_len >= MIN_CREATOR_PREFIX_LEN {
                            branches.push(format!("creator_tokens : {q}*"));
                        } else {
                            branches.push(format!("creator_tokens : {q}"));
                        }
                    }
                }

                if sub_tokens.len() >= 2 {
                    let sub_expr = sub_tokens
                        .iter()
                        .enumerate()
                        .map(|(idx, sub)| {
                            let sq = fts_quote(sub);
                            if idx + 1 == sub_tokens.len()
                                && !*exact_only
                                && sub.chars().count() >= MIN_PREFIX_LEN
                            {
                                format!("{sq}*")
                            } else {
                                sq
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    if include_creator {
                        branches.push(format!("{{title_tokens creator_tokens}} : ({sub_expr})"));
                    } else {
                        branches.push(format!("title_tokens : ({sub_expr})"));
                    }
                }

                if !*exact_only {
                    branches.extend(pinyin_fts_clause(compact));
                }

                term_clauses.push(format!("({})", branches.join(" OR ")));
            }
        }
    }

    let and_expr = term_clauses.join(" AND ");
    if let Some(ref wc) = query.whole_compact {
        let wq = fts_quote(wc);
        let mut wc_branches = vec![format!("{{title_tokens title_compact}} : {wq}*")];
        if include_creator {
            if wc.chars().count() >= MIN_CREATOR_PREFIX_LEN {
                wc_branches.push(format!("creator_tokens : {wq}*"));
            } else {
                wc_branches.push(format!("creator_tokens : {wq}"));
            }
        }
        wc_branches.extend(pinyin_fts_clause(wc));
        Some(format!("({and_expr}) OR ({})", wc_branches.join(" OR ")))
    } else {
        Some(and_expr)
    }
}

pub fn build_desc_fts_query(query: &ParsedQuery) -> Option<String> {
    if query.terms.is_empty() {
        return None;
    }

    let mut term_clauses = Vec::with_capacity(query.terms.len());
    for term in &query.terms {
        match term {
            QueryTerm::Cjk(chars) => {
                let phrase = cjk_phrase(chars);
                term_clauses.push(format!("desc_tokens : {phrase}"));
            }
            QueryTerm::Word {
                compact,
                sub_tokens,
                exact_only,
            } => {
                let q = fts_quote(compact);
                let base = if *exact_only || compact.chars().count() < MIN_PREFIX_LEN {
                    format!("{{desc_tokens desc_aliases}} : {q}")
                } else {
                    format!("{{desc_tokens desc_aliases}} : {q}*")
                };
                if sub_tokens.len() >= 2 {
                    let sub_expr = sub_tokens
                        .iter()
                        .enumerate()
                        .map(|(idx, sub)| {
                            let sq = fts_quote(sub);
                            if idx + 1 == sub_tokens.len()
                                && !*exact_only
                                && sub.chars().count() >= MIN_PREFIX_LEN
                            {
                                format!("{sq}*")
                            } else {
                                sq
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    term_clauses.push(format!("({base} OR desc_tokens : ({sub_expr}))"));
                } else {
                    term_clauses.push(base);
                }
            }
        }
    }

    let and_expr = term_clauses.join(" AND ");
    if let Some(ref wc) = query.whole_compact {
        let wq = fts_quote(wc);
        Some(format!(
            "({and_expr}) OR {{desc_tokens desc_aliases}} : {wq}*"
        ))
    } else {
        Some(and_expr)
    }
}

fn parse_query_terms(raw: &str, exact_only_words: &HashSet<String>) -> Vec<QueryTerm> {
    let mut terms = Vec::new();
    let mut cjk_buf: Vec<char> = Vec::new();
    let mut word_buf = String::new();

    let flush_cjk = |buf: &mut Vec<char>, out: &mut Vec<QueryTerm>| {
        if !buf.is_empty() {
            out.push(QueryTerm::Cjk(std::mem::take(buf)));
        }
    };

    let flush_word = |buf: &mut String, out: &mut Vec<QueryTerm>, exact_set: &HashSet<String>| {
        if buf.is_empty() {
            return;
        }
        let sub_tokens = split_word_run(buf);
        let compact: String = sub_tokens.concat();
        if !compact.is_empty() {
            let exact_only = exact_set.contains(&compact)
                || sub_tokens.iter().any(|s| exact_set.contains(s))
                || compact.chars().count() < MIN_PREFIX_LEN;
            out.push(QueryTerm::Word {
                compact,
                sub_tokens,
                exact_only,
            });
        }
        buf.clear();
    };

    for ch in raw.chars() {
        let ch = fold_char(ch);
        if is_cjk_split_char(ch) {
            flush_word(&mut word_buf, &mut terms, exact_only_words);
            cjk_buf.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk_buf, &mut terms);
            word_buf.push(ch);
        } else {
            flush_cjk(&mut cjk_buf, &mut terms);
            flush_word(&mut word_buf, &mut terms, exact_only_words);
        }
    }
    flush_cjk(&mut cjk_buf, &mut terms);
    flush_word(&mut word_buf, &mut terms, exact_only_words);
    const MAX_QUERY_TERMS: usize = 16;
    terms.truncate(MAX_QUERY_TERMS);
    terms
}

fn extract_steam_url_id(raw: &str) -> Option<u64> {
    let (scheme, _) = raw.split_once("://")?;
    if !["http", "https", "steam"]
        .iter()
        .any(|supported| scheme.eq_ignore_ascii_case(supported))
        || raw.contains('\\')
        || raw.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }

    let url = Url::parse(raw).ok()?;
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return None;
    }
    let host = url.host_str()?;
    let path = url.path().strip_suffix('/').unwrap_or(url.path());
    match url.scheme() {
        "http" | "https"
            if matches!(host, "steamcommunity.com" | "www.steamcommunity.com")
                && ["/sharedfiles/filedetails", "/workshop/filedetails"]
                    .iter()
                    .any(|workshop_path| path.eq_ignore_ascii_case(workshop_path)) =>
        {
            let (_, id) = url
                .query_pairs()
                .find(|(key, _)| key.eq_ignore_ascii_case("id"))?;
            parse_positive_id(&id)
        }
        "steam"
            if host.eq_ignore_ascii_case("url")
                && url.query().is_none()
                && url.fragment().is_none() =>
        {
            let (page, id) = path.rsplit_once('/')?;
            if !page.eq_ignore_ascii_case("/CommunityFilePage") {
                return None;
            }
            parse_positive_id(id)
        }
        _ => None,
    }
}

fn parse_positive_id(id: &str) -> Option<u64> {
    if !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse::<u64>().ok().filter(|id| *id > 0)
}

fn query_terms_compact_all(terms: &[QueryTerm]) -> String {
    let mut out = String::new();
    for term in terms {
        match term {
            QueryTerm::Cjk(chars) => {
                for &c in chars {
                    out.push(c);
                }
            }
            QueryTerm::Word { compact, .. } => out.push_str(compact),
        }
    }
    out
}

fn cjk_phrase(chars: &[char]) -> String {
    let mut inner = String::with_capacity(chars.len() * 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 {
            inner.push(' ');
        }
        inner.push(c);
    }
    format!("\"{inner}\"")
}

pub(super) fn fts_quote(token: &str) -> String {
    format!("\"{}\"", token.replace('"', ""))
}
