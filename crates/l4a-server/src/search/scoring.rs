use super::pinyin::{collect_han_pinyin_spans, match_pinyin_string, match_term_in_pinyin};
use super::text::{
    fold_text, is_cjk_split_char, is_pure_ascii_alpha, strip_bbcode_and_urls,
    tokenize_with_aliases, TokenizedText,
};
use super::{
    MatchRank, ParsedQuery, QueryTerm, Token, MAX_COMPACT_LEN, MAX_COMPACT_SPAN,
    MIN_COMPACT_PREFIX_LEN, MIN_CREATOR_PREFIX_LEN, MIN_PINYIN_LEN, MIN_PREFIX_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermMatch {
    Exact,
    Prefix,
}

pub fn score_primary_candidate(
    query: &ParsedQuery,
    workshop_id: u64,
    title: &str,
    creator_name: Option<&str>,
) -> Option<MatchRank> {
    if query.exact_workshop_id == Some(workshop_id) {
        return Some(MatchRank {
            tier: 0,
            sub_rank: 0,
        });
    }

    if query.terms.is_empty() {
        return if query.exact_workshop_id.is_some() {
            None
        } else {
            score_symbol_only_title(query, title)
        };
    }

    let title_folded = fold_text(title);
    if title_folded == query.raw_folded {
        return Some(MatchRank {
            tier: 0,
            sub_rank: 1,
        });
    }

    let title_tokens = tokenize_with_aliases(title);
    if query.contains_required_literals(&title_folded) {
        if let Some(rank) = score_title_direct(query, &title_folded, &title_tokens)
            .or_else(|| score_title_pinyin(query, title, &title_tokens))
        {
            return Some(rank);
        }
    }

    if let Some(ref prefix) = query.numeric_prefix {
        if workshop_id.to_string().starts_with(prefix) {
            return Some(MatchRank {
                tier: 5,
                sub_rank: 0,
            });
        }
    }

    if let Some(creator) = creator_name {
        let creator_folded = fold_text(creator);
        if !query.contains_required_literals(&creator_folded) {
            return None;
        }
        if creator_folded == query.raw_folded {
            return Some(MatchRank {
                tier: 5,
                sub_rank: 1,
            });
        }
        let creator_tokens = tokenize_with_aliases(creator);
        if matches_all_terms_in_tokens(query, &creator_tokens, MIN_CREATOR_PREFIX_LEN).is_some()
            || query.whole_compact.as_deref().is_some_and(|wc| {
                match_compact_in_tokens(wc, &creator_tokens, false, MIN_CREATOR_PREFIX_LEN)
                    .is_some()
            })
        {
            return Some(MatchRank {
                tier: 5,
                sub_rank: 2,
            });
        }

        // Cross-field author + title query (e.g. "perfect_buddy early")
        if query.terms.len() >= 2 && query.literal_substrings.is_empty() {
            let mut pinyin_spans = None;
            let all_matched = query.terms.iter().all(|term| {
                if match_term_in_tokens(term, &title_tokens, MIN_PREFIX_LEN).is_some()
                    || match_term_in_tokens(term, &creator_tokens, MIN_CREATOR_PREFIX_LEN).is_some()
                {
                    return true;
                }
                if !query.can_match_pinyin || title.is_ascii() {
                    return false;
                }
                let spans = pinyin_spans.get_or_insert_with(|| collect_han_pinyin_spans(title));
                match_term_in_pinyin(term, spans).is_some()
            });
            if all_matched {
                return Some(MatchRank {
                    tier: 5,
                    sub_rank: 3,
                });
            }
        }
    }

    None
}

pub fn score_title_only_candidate(query: &ParsedQuery, title: &str) -> Option<MatchRank> {
    if query.terms.is_empty() {
        return score_symbol_only_title(query, title);
    }

    let title_folded = fold_text(title);
    if title_folded == query.raw_folded {
        return Some(MatchRank {
            tier: 0,
            sub_rank: 1,
        });
    }

    let title_tokens = tokenize_with_aliases(title);
    if query.contains_required_literals(&title_folded) {
        if let Some(rank) = score_title_direct(query, &title_folded, &title_tokens)
            .or_else(|| score_title_pinyin(query, title, &title_tokens))
        {
            return Some(rank);
        }
    }

    None
}

pub fn score_symbol_only_title(query: &ParsedQuery, title: &str) -> Option<MatchRank> {
    let title = fold_text(title);
    if title == query.raw_folded {
        Some(MatchRank {
            tier: 0,
            sub_rank: 1,
        })
    } else if title.starts_with(&query.raw_folded) {
        Some(MatchRank {
            tier: 1,
            sub_rank: 0,
        })
    } else if title.contains(&query.raw_folded) {
        Some(MatchRank {
            tier: 2,
            sub_rank: 0,
        })
    } else {
        None
    }
}

pub fn score_symbol_only_desc(query: &ParsedQuery, description: &str) -> Option<MatchRank> {
    let cleaned = strip_bbcode_and_urls(description);
    fold_text(&cleaned)
        .contains(&query.raw_folded)
        .then_some(MatchRank {
            tier: 6,
            sub_rank: 0,
        })
}

pub fn score_desc_candidate(query: &ParsedQuery, description: &str) -> Option<MatchRank> {
    if query.terms.is_empty() {
        return score_symbol_only_desc(query, description);
    }

    let cleaned = strip_bbcode_and_urls(description);
    if !query.literal_substrings.is_empty()
        && !query.contains_required_literals(&fold_text(&cleaned))
    {
        return None;
    }

    let desc_tokens = tokenize_with_aliases(&cleaned);

    if let Some((_, kind)) = find_contiguous_query_match(query, &desc_tokens) {
        return Some(MatchRank {
            tier: 6,
            sub_rank: if kind == TermMatch::Exact { 0 } else { 1 },
        });
    }

    if let Some(kind) = matches_all_terms_in_tokens(query, &desc_tokens, MIN_PREFIX_LEN) {
        return Some(MatchRank {
            tier: 6,
            sub_rank: if kind == TermMatch::Exact { 1 } else { 2 },
        });
    }

    if let Some(ref wc) = query.whole_compact {
        if match_compact_in_tokens(wc, &desc_tokens, false, MIN_COMPACT_PREFIX_LEN).is_some() {
            return Some(MatchRank {
                tier: 6,
                sub_rank: 1,
            });
        }
    }

    None
}

fn score_title_direct(
    query: &ParsedQuery,
    title_folded: &str,
    title_tokens: &TokenizedText,
) -> Option<MatchRank> {
    if tokens_match_compact_all(&title_tokens.tokens, &query.compact_all) {
        return Some(MatchRank {
            tier: 0,
            sub_rank: 2,
        });
    }

    if let Some(rest) = title_folded.strip_prefix(&query.raw_folded) {
        let last_q = query.raw_folded.chars().next_back();
        let next_t = rest.chars().next();
        let at_word_boundary = match (last_q, next_t) {
            (_, None) => true,
            (Some(q), _) if is_cjk_split_char(q) || !q.is_alphanumeric() => true,
            (_, Some(n)) if is_cjk_split_char(n) || !n.is_alphanumeric() => true,
            _ => false,
        };
        if at_word_boundary {
            return Some(MatchRank {
                tier: 1,
                sub_rank: 0,
            });
        }
    }

    if let Some((start, kind)) = find_contiguous_query_match(query, title_tokens) {
        if start == 0 {
            return Some(MatchRank {
                tier: 1,
                sub_rank: if kind == TermMatch::Exact { 1 } else { 2 },
            });
        }
        if kind == TermMatch::Exact {
            return Some(MatchRank {
                tier: 2,
                sub_rank: 0,
            });
        }
    }

    if let Some(ref wc) = query.whole_compact {
        if let Some((start, kind)) =
            find_compact_in_tokens(wc, title_tokens, false, MIN_COMPACT_PREFIX_LEN)
        {
            if start == 0 {
                return Some(MatchRank {
                    tier: 1,
                    sub_rank: if kind == TermMatch::Exact { 1 } else { 2 },
                });
            }
            if kind == TermMatch::Exact {
                return Some(MatchRank {
                    tier: 2,
                    sub_rank: 1,
                });
            }
        }
    }

    if let Some(kind) = matches_all_terms_in_tokens(query, title_tokens, MIN_PREFIX_LEN) {
        return Some(MatchRank {
            tier: 3,
            sub_rank: if kind == TermMatch::Exact { 0 } else { 1 },
        });
    }

    if let Some(ref wc) = query.whole_compact {
        if find_compact_in_tokens(wc, title_tokens, false, MIN_COMPACT_PREFIX_LEN).is_some() {
            return Some(MatchRank {
                tier: 3,
                sub_rank: 1,
            });
        }
    }

    None
}

fn score_title_pinyin(
    query: &ParsedQuery,
    title: &str,
    title_tokens: &TokenizedText,
) -> Option<MatchRank> {
    if !query.can_match_pinyin || title.is_ascii() || !query.literal_substrings.is_empty() {
        return None;
    }

    let pinyin_spans = collect_han_pinyin_spans(title);
    if pinyin_spans.is_empty() {
        return None;
    }

    // Whole compact query (e.g. "guang zhou" -> "guangzhou", or single word "guangzhou")
    if let Some(ref wc) = query.whole_compact {
        if is_pure_ascii_alpha(wc) && wc.len() >= MIN_PINYIN_LEN {
            if let Some(starts_title) = match_pinyin_string(wc, &pinyin_spans) {
                return Some(MatchRank {
                    tier: 4,
                    sub_rank: if starts_title { 0 } else { 1 },
                });
            }
        }
    }

    if query.terms.len() == 1 {
        if let Some(starts_title) = match_term_in_pinyin(&query.terms[0], &pinyin_spans) {
            return Some(MatchRank {
                tier: 4,
                sub_rank: if starts_title { 0 } else { 1 },
            });
        }
        return None;
    }

    // Multi-term mixed direct + pinyin query (e.g. "guangzhou 8.06", "guangzhou zengcheng")
    let mut used_pinyin = false;
    let mut starts_title = false;
    for (idx, term) in query.terms.iter().enumerate() {
        if match_term_in_tokens(term, title_tokens, MIN_PREFIX_LEN).is_some() {
            continue;
        }
        if let Some(at_start) = match_term_in_pinyin(term, &pinyin_spans) {
            used_pinyin = true;
            if idx == 0 && at_start {
                starts_title = true;
            }
            continue;
        }
        return None;
    }

    if used_pinyin {
        Some(MatchRank {
            tier: 4,
            sub_rank: if starts_title { 0 } else { 1 },
        })
    } else {
        None
    }
}

fn find_contiguous_query_match(
    query: &ParsedQuery,
    text: &TokenizedText,
) -> Option<(usize, TermMatch)> {
    let tokens = &text.tokens;
    // 1. Check whole-query compact match at token boundaries first for single-word queries
    //    (e.g. "DayBreak" / "daybreak" against ["day", "break", "v2"]).
    if query.terms.len() == 1 {
        return match &query.terms[0] {
            QueryTerm::Cjk(chars) => {
                find_cjk_seq_in_tokens(chars, tokens).map(|pos| (pos, TermMatch::Exact))
            }
            QueryTerm::Word {
                compact,
                sub_tokens,
                exact_only,
            } => {
                let mut best = find_word_in_tokens(compact, tokens, *exact_only, MIN_PREFIX_LEN);
                if best == Some((0, TermMatch::Exact)) {
                    return best;
                }
                if let Some(hit) =
                    find_compact_in_tokens(compact, text, *exact_only, MIN_COMPACT_PREFIX_LEN)
                {
                    best = pick_better_pos_match(best, hit);
                }
                if sub_tokens.len() >= 2 {
                    if let Some(hit) = find_word_seq_in_tokens(sub_tokens, tokens, *exact_only) {
                        best = pick_better_pos_match(best, hit);
                    }
                }
                best
            }
        };
    }

    // 2. Multi-term contiguous sequence match (e.g. "Early Days", "Chernobyl Part 2", "广州 增城")
    let mut flat_exact_or_prefix = Vec::new();
    for term in &query.terms {
        match term {
            QueryTerm::Cjk(chars) => {
                for &c in chars {
                    flat_exact_or_prefix.push((Token::Cjk(c), true));
                }
            }
            QueryTerm::Word {
                sub_tokens,
                exact_only,
                ..
            } => {
                for sub in sub_tokens {
                    flat_exact_or_prefix.push((Token::Word(sub.clone()), *exact_only));
                }
            }
        }
    }

    if flat_exact_or_prefix.is_empty() || flat_exact_or_prefix.len() > tokens.len() {
        return None;
    }

    let mut best: Option<(usize, TermMatch)> = None;
    for start in 0..=(tokens.len() - flat_exact_or_prefix.len()) {
        let mut kind = TermMatch::Exact;
        let mut ok = true;
        for (offset, (qtok, exact_only)) in flat_exact_or_prefix.iter().enumerate() {
            let dtok = &tokens[start + offset];
            let is_last = offset + 1 == flat_exact_or_prefix.len();
            match (qtok, dtok) {
                (Token::Cjk(qc), Token::Cjk(dc)) if qc == dc => {}
                (Token::Word(qw), Token::Word(dw)) if qw == dw => {}
                (Token::Word(qw), Token::Word(dw))
                    if is_last
                        && !*exact_only
                        && qw.chars().count() >= MIN_PREFIX_LEN
                        && dw.starts_with(qw.as_str()) =>
                {
                    kind = TermMatch::Prefix;
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            best = pick_better_pos_match(best, (start, kind));
            if start == 0 && kind == TermMatch::Exact {
                break;
            }
        }
    }
    best
}

fn pick_better_pos_match(
    current: Option<(usize, TermMatch)>,
    candidate: (usize, TermMatch),
) -> Option<(usize, TermMatch)> {
    let rank = |(pos, kind): (usize, TermMatch)| {
        (
            if pos == 0 { 0u8 } else { 1u8 },
            if kind == TermMatch::Exact { 0u8 } else { 1u8 },
            pos,
        )
    };
    match current {
        Some(cur) if rank(cur) <= rank(candidate) => Some(cur),
        _ => Some(candidate),
    }
}

fn matches_all_terms_in_tokens(
    query: &ParsedQuery,
    tokens: &TokenizedText,
    min_prefix_len: usize,
) -> Option<TermMatch> {
    if query.terms.is_empty() {
        return None;
    }
    let mut overall = TermMatch::Exact;
    for term in &query.terms {
        match match_term_in_tokens(term, tokens, min_prefix_len)? {
            TermMatch::Exact => {}
            TermMatch::Prefix => overall = TermMatch::Prefix,
        }
    }
    Some(overall)
}

fn match_term_in_tokens(
    term: &QueryTerm,
    text: &TokenizedText,
    min_prefix_len: usize,
) -> Option<TermMatch> {
    let tokens = &text.tokens;
    match term {
        QueryTerm::Cjk(chars) => find_cjk_seq_in_tokens(chars, tokens).map(|_| TermMatch::Exact),
        QueryTerm::Word {
            compact,
            sub_tokens,
            exact_only,
        } => {
            if let Some((_, kind)) =
                find_word_in_tokens(compact, tokens, *exact_only, min_prefix_len)
            {
                if kind == TermMatch::Exact {
                    return Some(TermMatch::Exact);
                }
                if find_compact_in_tokens(compact, text, *exact_only, MIN_COMPACT_PREFIX_LEN)
                    .is_some_and(|(_, k)| k == TermMatch::Exact)
                {
                    return Some(TermMatch::Exact);
                }
                return Some(TermMatch::Prefix);
            }

            if let Some((_, kind)) =
                find_compact_in_tokens(compact, text, *exact_only, MIN_COMPACT_PREFIX_LEN)
            {
                return Some(kind);
            }

            if sub_tokens.len() >= 2 {
                let mut kind = TermMatch::Exact;
                for (idx, sub) in sub_tokens.iter().enumerate() {
                    let sub_exact = *exact_only || idx + 1 < sub_tokens.len();
                    match find_word_in_tokens(sub, tokens, sub_exact, min_prefix_len) {
                        Some((_, TermMatch::Exact)) => {}
                        Some((_, TermMatch::Prefix)) => kind = TermMatch::Prefix,
                        None => return None,
                    }
                }
                return Some(kind);
            }

            None
        }
    }
}

fn find_cjk_seq_in_tokens(needle: &[char], tokens: &[Token]) -> Option<usize> {
    if needle.is_empty() || needle.len() > tokens.len() {
        return None;
    }
    (0..=(tokens.len() - needle.len())).find(|&start| {
        needle
            .iter()
            .enumerate()
            .all(|(i, &c)| tokens[start + i].as_cjk() == Some(c))
    })
}

fn find_word_in_tokens(
    needle: &str,
    tokens: &[Token],
    exact_only: bool,
    min_prefix_len: usize,
) -> Option<(usize, TermMatch)> {
    let allow_prefix = !exact_only && needle.chars().count() >= min_prefix_len;
    let mut best: Option<(usize, TermMatch)> = None;
    for (idx, tok) in tokens.iter().enumerate() {
        let Some(word) = tok.as_word() else {
            continue;
        };
        if word == needle {
            best = pick_better_pos_match(best, (idx, TermMatch::Exact));
            if idx == 0 {
                break;
            }
        } else if allow_prefix && word.starts_with(needle) {
            best = pick_better_pos_match(best, (idx, TermMatch::Prefix));
        }
    }
    best
}

fn find_word_seq_in_tokens(
    seq: &[String],
    tokens: &[Token],
    exact_only: bool,
) -> Option<(usize, TermMatch)> {
    if seq.is_empty() || seq.len() > tokens.len() {
        return None;
    }
    let mut best: Option<(usize, TermMatch)> = None;
    for start in 0..=(tokens.len() - seq.len()) {
        let mut kind = TermMatch::Exact;
        let mut ok = true;
        for (off, q) in seq.iter().enumerate() {
            let Some(dw) = tokens[start + off].as_word() else {
                ok = false;
                break;
            };
            let is_last = off + 1 == seq.len();
            if dw == q {
                continue;
            }
            if is_last && !exact_only && q.chars().count() >= MIN_PREFIX_LEN && dw.starts_with(q) {
                kind = TermMatch::Prefix;
                continue;
            }
            ok = false;
            break;
        }
        if ok {
            best = pick_better_pos_match(best, (start, kind));
        }
    }
    best
}

fn match_compact_in_tokens(
    needle: &str,
    tokens: &TokenizedText,
    exact_only: bool,
    min_prefix_len: usize,
) -> Option<TermMatch> {
    find_compact_in_tokens(needle, tokens, exact_only, min_prefix_len).map(|(_, kind)| kind)
}

/// Matches an original word or a bounded span of adjacent word tokens.
fn find_compact_in_tokens(
    needle: &str,
    text: &TokenizedText,
    exact_only: bool,
    min_prefix_len: usize,
) -> Option<(usize, TermMatch)> {
    let allow_prefix = !exact_only && needle.chars().count() >= min_prefix_len;
    let mut best = None;
    for (start, word) in &text.unsplit_words {
        if word == needle {
            best = pick_better_pos_match(best, (*start, TermMatch::Exact));
            if *start == 0 {
                return best;
            }
        } else if allow_prefix && word.starts_with(needle) {
            best = pick_better_pos_match(best, (*start, TermMatch::Prefix));
        }
    }

    let tokens = &text.tokens;
    for start in 0..tokens.len() {
        let mut remaining = needle;
        let mut joined_len = 0;
        let max_end = (start + MAX_COMPACT_SPAN).min(tokens.len());
        for tok in &tokens[start..max_end] {
            let Some(w) = tok.as_word() else {
                break;
            };
            joined_len += w.len();
            if joined_len > MAX_COMPACT_LEN {
                break;
            }
            let Some(rest) = remaining.strip_prefix(w) else {
                if allow_prefix && w.starts_with(remaining) {
                    best = pick_better_pos_match(best, (start, TermMatch::Prefix));
                }
                break;
            };
            if rest.is_empty() {
                best = pick_better_pos_match(best, (start, TermMatch::Exact));
                if start == 0 {
                    return best;
                }
                break;
            }
            remaining = rest;
        }
    }

    best
}

fn tokens_match_compact_all(tokens: &[Token], mut compact: &str) -> bool {
    for tok in tokens {
        let remainder = match tok {
            Token::Word(w) => compact.strip_prefix(w.as_str()),
            Token::Cjk(c) => compact.strip_prefix(*c),
        };
        let Some(remainder) = remainder else {
            return false;
        };
        compact = remainder;
    }
    compact.is_empty()
}
