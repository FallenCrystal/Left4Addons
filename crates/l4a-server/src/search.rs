use pinyin::ToPinyinMulti;
use std::collections::HashSet;

pub const SEARCH_INDEX_VERSION: &str = "1";

const MAX_COMPACT_SPAN: usize = 4;
const MAX_COMPACT_LEN: usize = 40;
const MAX_PINYIN_SPAN: usize = 6;
const MAX_PINYIN_VARIANTS_PER_SPAN: usize = 6;
const MIN_PREFIX_LEN: usize = 2;
const MIN_COMPACT_PREFIX_LEN: usize = 4;
const MIN_CREATOR_PREFIX_LEN: usize = 4;
const MIN_PINYIN_LEN: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Cjk(char),
}

impl Token {
    fn as_word(&self) -> Option<&str> {
        match self {
            Self::Word(w) => Some(w.as_str()),
            Self::Cjk(_) => None,
        }
    }

    fn as_cjk(&self) -> Option<char> {
        match self {
            Self::Cjk(c) => Some(*c),
            Self::Word(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedDocument {
    pub title_tokens: String,
    pub title_compact: String,
    pub title_pinyin: String,
    pub creator_tokens: String,
    pub desc_tokens: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryTerm {
    Cjk(Vec<char>),
    Word {
        compact: String,
        sub_tokens: Vec<String>,
        exact_only: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub raw_trimmed: String,
    pub raw_folded: String,
    pub exact_workshop_id: Option<u64>,
    pub numeric_prefix: Option<String>,
    pub terms: Vec<QueryTerm>,
    pub whole_compact: Option<String>,
    pub literal_substrings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchRank {
    pub tier: u8,
    pub sub_rank: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermMatch {
    Exact,
    Prefix,
}

pub fn build_indexed_document(
    title: &str,
    creator_name: Option<&str>,
    description: Option<&str>,
) -> IndexedDocument {
    let title_toks = tokenize_text(title);
    let title_tokens = tokens_to_fts_string(&title_toks);
    let title_compact = build_compact_tokens(title, &title_toks, MAX_COMPACT_SPAN);
    let title_pinyin = build_pinyin_tokens(title);

    let creator_tokens = creator_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|creator| {
            let toks = tokenize_text(creator);
            let base = tokens_to_fts_string(&toks);
            let compact = build_compact_tokens(creator, &toks, 3);
            if compact.is_empty() {
                base
            } else if base.is_empty() {
                compact
            } else {
                format!("{base} {compact}")
            }
        })
        .unwrap_or_default();

    let desc_tokens = description
        .map(strip_bbcode_and_urls)
        .map(|cleaned| tokens_to_fts_string(&tokenize_text(&cleaned)))
        .unwrap_or_default();

    IndexedDocument {
        title_tokens,
        title_compact,
        title_pinyin,
        creator_tokens,
        desc_tokens,
    }
}

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

    let all_words = !terms.is_empty()
        && terms.iter().all(|t| {
            matches!(
                t,
                QueryTerm::Word {
                    exact_only: false,
                    ..
                } | QueryTerm::Word { .. }
            )
        });
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
            let joined: String = terms
                .iter()
                .filter_map(|t| match t {
                    QueryTerm::Word { compact, .. } => Some(compact.as_str()),
                    QueryTerm::Cjk(_) => None,
                })
                .collect();
            if joined.len() <= MAX_COMPACT_LEN {
                Some(joined)
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
    })
}

pub fn build_primary_fts_query(query: &ParsedQuery) -> Option<String> {
    if query.terms.is_empty() {
        return None;
    }

    let mut term_clauses = Vec::with_capacity(query.terms.len());
    for term in &query.terms {
        match term {
            QueryTerm::Cjk(chars) => {
                let phrase = cjk_phrase(chars);
                term_clauses.push(format!("{{title_tokens creator_tokens}} : {phrase}"));
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
                    branches.push(format!(
                        "{{title_tokens title_compact creator_tokens}} : {q}"
                    ));
                } else {
                    branches.push(format!("title_tokens : {q}*"));
                    if char_len >= MIN_COMPACT_PREFIX_LEN {
                        branches.push(format!("title_compact : {q}*"));
                    } else {
                        branches.push(format!("title_compact : {q}"));
                    }
                    if char_len >= MIN_CREATOR_PREFIX_LEN {
                        branches.push(format!("creator_tokens : {q}*"));
                    } else {
                        branches.push(format!("creator_tokens : {q}"));
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
                    branches.push(format!("{{title_tokens creator_tokens}} : ({sub_expr})"));
                }

                if !*exact_only && is_pure_ascii_alpha(compact) && compact.len() >= MIN_PINYIN_LEN {
                    branches.push(format!("title_pinyin : {q}"));
                }

                term_clauses.push(format!("({})", branches.join(" OR ")));
            }
        }
    }

    let and_expr = term_clauses.join(" AND ");
    if let Some(ref wc) = query.whole_compact {
        let wq = fts_quote(wc);
        let mut wc_branches = vec![format!("{{title_tokens title_compact}} : {wq}*")];
        if wc.chars().count() >= MIN_CREATOR_PREFIX_LEN {
            wc_branches.push(format!("creator_tokens : {wq}*"));
        } else {
            wc_branches.push(format!("creator_tokens : {wq}"));
        }
        if is_pure_ascii_alpha(wc) && wc.len() >= MIN_PINYIN_LEN {
            wc_branches.push(format!("title_pinyin : {wq}"));
        }
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
                    format!("desc_tokens : {q}")
                } else {
                    format!("desc_tokens : {q}*")
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
        Some(format!("({and_expr}) OR desc_tokens : {wq}*"))
    } else {
        Some(and_expr)
    }
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
        return None;
    }

    let title_folded = fold_text(title);
    let creator_folded = creator_name.map(fold_text);
    let title_has_literals = query.literal_substrings.is_empty()
        || query
            .literal_substrings
            .iter()
            .all(|s| title_folded.contains(s));
    let creator_has_literals = creator_folded.as_deref().is_some_and(|c| {
        query.literal_substrings.is_empty()
            || query.literal_substrings.iter().all(|s| c.contains(s))
    });

    if !title_has_literals && !creator_has_literals {
        return None;
    }

    let title_tokens = tokenize_text(title);
    let mut best: Option<MatchRank> = None;

    if title_has_literals {
        if let Some(rank) = score_title_direct(query, &title_folded, &title_tokens) {
            best = Some(best.map_or(rank, |cur| cur.min(rank)));
        }

        if best.is_none() || best.is_some_and(|r| r.tier > 4) {
            if let Some(rank) = score_title_pinyin(query, title, &title_tokens) {
                best = Some(best.map_or(rank, |cur| cur.min(rank)));
            }
        }
    }

    if best.is_some() {
        return best;
    }

    if let Some(ref prefix) = query.numeric_prefix {
        if workshop_id.to_string().starts_with(prefix) {
            return Some(MatchRank {
                tier: 5,
                sub_rank: 0,
            });
        }
    }

    if let (Some(creator), Some(c_folded)) = (creator_name, creator_folded.as_deref()) {
        let creator_tokens = tokenize_text(creator);
        if creator_has_literals {
            if c_folded == query.raw_folded {
                return Some(MatchRank {
                    tier: 5,
                    sub_rank: 1,
                });
            }
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
        }

        // Cross-field author + title query (e.g. "perfect_buddy early")
        if query.terms.len() >= 2 && query.literal_substrings.is_empty() {
            let pinyin_spans = collect_han_pinyin_spans(title);
            let all_matched = query.terms.iter().all(|term| {
                match_term_in_tokens(term, &title_tokens, MIN_PREFIX_LEN).is_some()
                    || match_term_in_tokens(term, &creator_tokens, MIN_CREATOR_PREFIX_LEN).is_some()
                    || match_term_in_pinyin(term, &pinyin_spans).is_some()
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

pub fn score_desc_candidate(
    query: &ParsedQuery,
    indexed_desc_tokens: &str,
    raw_description: Option<&str>,
) -> Option<MatchRank> {
    if indexed_desc_tokens.is_empty() || query.terms.is_empty() {
        return None;
    }

    if !query.literal_substrings.is_empty() {
        let desc = raw_description?;
        let desc_folded = fold_text(desc);
        if !query
            .literal_substrings
            .iter()
            .all(|s| desc_folded.contains(s))
        {
            return None;
        }
    }

    let desc_tokens = parse_indexed_tokens(indexed_desc_tokens);

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

fn parse_indexed_tokens(indexed: &str) -> Vec<Token> {
    indexed
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                if is_cjk_split_char(ch) {
                    return Token::Cjk(ch);
                }
            }
            Token::Word(part.to_string())
        })
        .collect()
}

fn score_title_direct(
    query: &ParsedQuery,
    title_folded: &str,
    title_tokens: &[Token],
) -> Option<MatchRank> {
    if title_folded == query.raw_folded {
        return Some(MatchRank {
            tier: 0,
            sub_rank: 1,
        });
    }

    let title_compact_all = tokens_compact_all(title_tokens);
    let query_compact_all = query_terms_compact_all(&query.terms);
    if !title_compact_all.is_empty() && title_compact_all == query_compact_all {
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
    title_tokens: &[Token],
) -> Option<MatchRank> {
    if !query.literal_substrings.is_empty() {
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
    tokens: &[Token],
) -> Option<(usize, TermMatch)> {
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
                let mut best: Option<(usize, TermMatch)> = None;
                if let Some(hit) =
                    find_compact_in_tokens(compact, tokens, *exact_only, MIN_COMPACT_PREFIX_LEN)
                {
                    best = pick_better_pos_match(best, hit);
                }
                if let Some(hit) = find_word_in_tokens(compact, tokens, *exact_only, MIN_PREFIX_LEN)
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
    tokens: &[Token],
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
    tokens: &[Token],
    min_prefix_len: usize,
) -> Option<TermMatch> {
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
                if find_compact_in_tokens(compact, tokens, *exact_only, MIN_COMPACT_PREFIX_LEN)
                    .is_some_and(|(_, k)| k == TermMatch::Exact)
                {
                    return Some(TermMatch::Exact);
                }
                return Some(TermMatch::Prefix);
            }

            if let Some((_, kind)) =
                find_compact_in_tokens(compact, tokens, *exact_only, MIN_COMPACT_PREFIX_LEN)
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
    tokens: &[Token],
    exact_only: bool,
    min_prefix_len: usize,
) -> Option<TermMatch> {
    find_compact_in_tokens(needle, tokens, exact_only, min_prefix_len).map(|(_, kind)| kind)
}

/// Checks if `needle` matches a contiguous span of 2..=MAX_COMPACT_SPAN word tokens
/// starting at a token boundary in `tokens`.
fn find_compact_in_tokens(
    needle: &str,
    tokens: &[Token],
    exact_only: bool,
    min_prefix_len: usize,
) -> Option<(usize, TermMatch)> {
    let allow_prefix = !exact_only && needle.chars().count() >= min_prefix_len;
    let mut best: Option<(usize, TermMatch)> = None;
    let mut buf = String::new();

    for start in 0..tokens.len() {
        if tokens[start].as_word().is_none() {
            continue;
        }
        buf.clear();
        let max_end = (start + MAX_COMPACT_SPAN).min(tokens.len());
        for (span_len, tok) in tokens[start..max_end].iter().enumerate() {
            let Some(w) = tok.as_word() else {
                break;
            };
            if buf.len() + w.len() > MAX_COMPACT_LEN {
                break;
            }
            buf.push_str(w);
            if span_len == 0 {
                // Single token handled by `find_word_in_tokens`, except when called via `whole_compact`
                if buf == needle {
                    best = pick_better_pos_match(best, (start, TermMatch::Exact));
                } else if allow_prefix && buf.starts_with(needle) {
                    best = pick_better_pos_match(best, (start, TermMatch::Prefix));
                }
                continue;
            }
            if buf == needle {
                best = pick_better_pos_match(best, (start, TermMatch::Exact));
                if start == 0 {
                    return best;
                }
            } else if allow_prefix && buf.starts_with(needle) {
                best = pick_better_pos_match(best, (start, TermMatch::Prefix));
            }
            if buf.len() >= needle.len() && !buf.starts_with(needle) {
                break;
            }
        }
    }

    best
}

#[derive(Debug)]
struct HanPinyinSpan {
    starts_title: bool,
    readings: Vec<Vec<String>>,
}

fn collect_han_pinyin_spans(title: &str) -> Vec<HanPinyinSpan> {
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

fn match_term_in_pinyin(term: &QueryTerm, spans: &[HanPinyinSpan]) -> Option<bool> {
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
fn match_pinyin_string(needle: &str, spans: &[HanPinyinSpan]) -> Option<bool> {
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

fn build_pinyin_tokens(title: &str) -> String {
    let spans = collect_han_pinyin_spans(title);
    if spans.is_empty() {
        return String::new();
    }

    let mut emitted = HashSet::new();
    let mut out = Vec::new();
    for span in &spans {
        let n = span.readings.len();
        for start in 0..n {
            let max_end = (start + MAX_PINYIN_SPAN).min(n);
            let mut prefixes = vec![String::new()];
            for (offset, char_readings) in span.readings[start..max_end].iter().enumerate() {
                let mut next = Vec::new();
                'outer: for prefix in &prefixes {
                    for reading in char_readings {
                        let mut combined = String::with_capacity(prefix.len() + reading.len());
                        combined.push_str(prefix);
                        combined.push_str(reading);
                        if combined.len() <= MAX_COMPACT_LEN {
                            next.push(combined);
                            if next.len() >= MAX_PINYIN_VARIANTS_PER_SPAN {
                                break 'outer;
                            }
                        }
                    }
                }
                prefixes = next;
                if offset >= 1 {
                    for candidate in &prefixes {
                        if emitted.insert(candidate.clone()) {
                            out.push(candidate.clone());
                        }
                    }
                }
            }
        }
    }
    out.join(" ")
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

fn build_compact_tokens(raw: &str, tokens: &[Token], max_span: usize) -> String {
    let mut emitted = HashSet::new();
    let mut out = Vec::new();
    let mut buf = String::new();

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

    // Preserve very long unsplit CamelCase runs (> max_span pieces) as compact tokens too.
    let mut run = String::new();
    for ch in raw.chars() {
        let ch = fold_char(ch);
        if ch.is_alphanumeric() && !is_cjk_split_char(ch) {
            run.push(ch);
        } else {
            if !run.is_empty() {
                let lowered = fold_text(&run);
                let pieces = split_word_run(&run);
                if pieces.len() > max_span
                    && lowered.len() <= MAX_COMPACT_LEN
                    && emitted.insert(lowered.clone())
                {
                    out.push(lowered);
                }
                run.clear();
            }
        }
    }
    if !run.is_empty() {
        let lowered = fold_text(&run);
        let pieces = split_word_run(&run);
        if pieces.len() > max_span
            && lowered.len() <= MAX_COMPACT_LEN
            && emitted.insert(lowered.clone())
        {
            out.push(lowered);
        }
    }

    out.join(" ")
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
    terms
}

pub fn tokenize_text(raw: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word_buf = String::new();

    let flush_word = |buf: &mut String, out: &mut Vec<Token>| {
        if buf.is_empty() {
            return;
        }
        for sub in split_word_run(buf) {
            if !sub.is_empty() {
                out.push(Token::Word(sub));
            }
        }
        buf.clear();
    };

    for ch in raw.chars() {
        let ch = fold_char(ch);
        if is_cjk_split_char(ch) {
            flush_word(&mut word_buf, &mut tokens);
            tokens.push(Token::Cjk(ch));
        } else if ch.is_alphanumeric() {
            word_buf.push(ch);
        } else {
            flush_word(&mut word_buf, &mut tokens);
        }
    }
    flush_word(&mut word_buf, &mut tokens);
    tokens
}

/// Splits an alphanumeric run on CamelCase transitions (e.g. `DayBreak` -> `["day", "break"]`,
/// `YiChangCity2` -> `["yi", "chang", "city", "2"]`, `HTMLParser` -> `["html", "parser"]`) while
/// preserving single-letter suffixes like `MissionX` and compact codes like `L4D2`, `M60`, `v2`.
fn split_word_run(run: &str) -> Vec<String> {
    let chars: Vec<char> = run.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut camel_parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let curr = chars[i];
        let next = chars.get(i + 1).copied();

        let lower_to_upper_word =
            prev.is_lowercase() && curr.is_uppercase() && next.is_some_and(char::is_lowercase);
        let acronym_to_word =
            prev.is_uppercase() && curr.is_uppercase() && next.is_some_and(char::is_lowercase);
        let digit_to_upper_word =
            prev.is_ascii_digit() && curr.is_uppercase() && next.is_some_and(char::is_lowercase);

        if lower_to_upper_word || acronym_to_word || digit_to_upper_word {
            camel_parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    camel_parts.push(chars[start..].iter().collect());

    let mut out = Vec::with_capacity(camel_parts.len());
    for part in camel_parts {
        let lowered = fold_text(&part);
        // Split trailing digits off long words (e.g. "city2" -> "city", "2"; "part1" -> "part", "1";
        // "cyberpunk2077" -> "cyberpunk", "2077"), while keeping codes like "l4d2", "m60", "v2", "lv8".
        if let Some((prefix, digits)) = split_long_word_trailing_digits(&lowered) {
            out.push(prefix.to_string());
            out.push(digits.to_string());
        } else if !lowered.is_empty() {
            out.push(lowered);
        }
    }
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

pub fn strip_bbcode_and_urls(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    let mut skip_until_close: Option<&'static str> = None;

    while i < bytes.len() {
        if let Some(close_tag) = skip_until_close {
            if let Some(rel) = find_ascii_case_insensitive(&raw[i..], close_tag) {
                i += rel + close_tag.len();
                skip_until_close = None;
                out.push(' ');
                continue;
            }
            break;
        }

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
                    let is_closing = trimmed_tag.starts_with('/');
                    if !is_closing && lower_name == "img" {
                        skip_until_close = Some("[/img]");
                    } else if !is_closing && lower_name == "url" && !trimmed_tag.contains('=') {
                        skip_until_close = Some("[/url]");
                    } else if !is_closing && lower_name == "previewyoutube" {
                        skip_until_close = Some("[/previewyoutube]");
                    }
                    i += close_rel + 1;
                    out.push(' ');
                    continue;
                }
            }
        }

        // Strip bare http(s):// URLs
        if raw[i..].starts_with("http://")
            || raw[i..].starts_with("https://")
            || raw[i..].starts_with("HTTP://")
            || raw[i..].starts_with("HTTPS://")
        {
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b']' | b')' | b'>' | b'"' | b'\'')
            {
                i += 1;
            }
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

fn extract_steam_url_id(raw: &str) -> Option<u64> {
    let lower = raw.to_ascii_lowercase();
    if !lower.contains("steamcommunity.com/") {
        return None;
    }
    let idx = lower.find("?id=").or_else(|| lower.find("&id="))?;
    let rest = &raw[idx + 4..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok().filter(|id| *id > 0)
}

fn tokens_to_fts_string(tokens: &[Token]) -> String {
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

fn tokens_compact_all(tokens: &[Token]) -> String {
    let mut out = String::new();
    for tok in tokens {
        match tok {
            Token::Word(w) => out.push_str(w),
            Token::Cjk(c) => out.push(*c),
        }
    }
    out
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

fn fts_quote(token: &str) -> String {
    format!("\"{}\"", token.replace('"', ""))
}

pub fn fold_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let ch = fold_char(ch);
        for lc in ch.to_lowercase() {
            out.push(fold_latin_diacritic(lc));
        }
    }
    out
}

fn fold_char(ch: char) -> char {
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

fn is_han(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF
    )
}

fn is_kana(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF)
}

fn is_cjk_split_char(ch: char) -> bool {
    is_han(ch) || is_kana(ch)
}

fn is_pure_ascii_alpha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_and_camel_case_matching_finds_daybreak_and_earlydays() {
        let day_break_doc = build_indexed_document("Day Break v2", Some("Hoangzp"), None);
        assert!(day_break_doc.title_compact.contains("daybreak"));

        for query_str in ["DayBreak", "daybreak", "Day Break", "day_break", "daybre"] {
            let q = parse_query(query_str).unwrap();
            let rank = score_primary_candidate(&q, 2750097524, "Day Break v2", Some("Hoangzp"));
            assert!(rank.is_some(), "expected {query_str} to match Day Break v2");
            assert!(
                rank.unwrap().tier <= 1,
                "expected top tier for {query_str}, got {rank:?}"
            );
        }

        // Must NOT match mid-word boundary like "Friday Breaking"
        let q = parse_query("daybreak").unwrap();
        assert!(score_primary_candidate(&q, 1, "Friday Breaking", None).is_none());

        // EarlyDays matches ALL parts via title without relying on description
        let q = parse_query("EarlyDays").unwrap();
        for part in 1..=6 {
            let title = format!("Early Days PART {part}/6");
            let rank = score_primary_candidate(&q, 100 + part, &title, Some("perfect_buddy"));
            assert_eq!(
                rank,
                Some(MatchRank {
                    tier: 1,
                    sub_rank: 1
                })
            );
        }
    }

    #[test]
    fn chinese_pinyin_matches_guangzhou_and_yichang_without_english_false_positives() {
        let gz_title = "广州增城 （Zengcheng）Lv8.06";
        let gz_doc = build_indexed_document(gz_title, Some("长远的梦"), None);
        assert!(gz_doc.title_pinyin.contains("guangzhou"));
        assert!(gz_doc.title_pinyin.contains("zengcheng"));
        assert!(gz_doc.title_pinyin.contains("guangzhouzengcheng"));

        for q_str in [
            "guangzhou",
            "guang zhou",
            "guangzhouzengcheng",
            "广州",
            "增城",
            "zengcheng",
        ] {
            let q = parse_query(q_str).unwrap();
            let rank = score_primary_candidate(&q, 2396847377, gz_title, Some("长远的梦"));
            assert!(rank.is_some(), "expected {q_str} to match {gz_title}");
        }

        // 宜昌市V5.01 has no English in title but matches via pinyin
        let yc_q = parse_query("yichang").unwrap();
        assert_eq!(
            score_primary_candidate(&yc_q, 2802325803, "宜昌市V5.01", Some("JohNny.R")),
            Some(MatchRank {
                tier: 4,
                sub_rank: 0
            })
        );

        // Polyphone city names & ü -> v spelling
        for (q_str, title) in [
            ("chongqing", "重庆洪崖洞"),
            ("xiamen", "厦门鼓浪屿"),
            ("chengdu", "成都春熙路"),
            ("nvwu", "月光女巫替换"),
        ] {
            let q = parse_query(q_str).unwrap();
            let doc = build_indexed_document(title, None, None);
            assert!(
                doc.title_pinyin.split_whitespace().any(|t| t == q_str),
                "expected {q_str} in pinyin index for {title}: {}",
                doc.title_pinyin
            );
            assert!(score_primary_candidate(&q, 1, title, None).is_some());
        }

        // Single-syllable English words must NOT match single Chinese characters via pinyin
        for (q_str, title) in [("man", "满血复活"), ("gun", "铁棍武器"), ("die", "蝴蝶刀")]
        {
            let q = parse_query(q_str).unwrap();
            assert!(
                score_primary_candidate(&q, 1, title, None).is_none(),
                "{q_str} should not false-match {title}"
            );
        }
    }

    #[test]
    fn literal_underscore_suffix_rejects_non_matching_word() {
        let q = parse_query("mission_").unwrap();
        assert!(score_primary_candidate(&q, 9001, "MISSION_", None).is_some());
        assert!(score_primary_candidate(&q, 9002, "mission_ Zulu", None).is_some());
        assert!(score_primary_candidate(&q, 9004, "An earlier Mission_ text", None).is_some());
        assert!(score_primary_candidate(&q, 9007, "MissionX", None).is_none());
    }

    #[test]
    fn bbcode_stripping_removes_urls_and_img_tags_while_keeping_anchor_text() {
        let raw = "[img]https://imgur.com/earlydays.png[/img] [b]Check[/b] [url=https://steamcommunity.com/sharedfiles/filedetails/?id=123]Early Days[/url] https://example.com/daybreak";
        let cleaned = strip_bbcode_and_urls(raw);
        assert!(!cleaned.contains("imgur"));
        assert!(!cleaned.contains("steamcommunity"));
        assert!(!cleaned.contains("example.com"));
        assert!(cleaned.contains("Check"));
        assert!(cleaned.contains("Early Days"));
    }

    #[test]
    fn steam_workshop_url_extracts_exact_id() {
        let q = parse_query(
            "https://steamcommunity.com/sharedfiles/filedetails/?id=2396847377&searchtext=foo",
        )
        .unwrap();
        assert_eq!(q.exact_workshop_id, Some(2396847377));
        assert!(q.terms.is_empty());
        assert_eq!(
            score_primary_candidate(&q, 2396847377, "广州增城", None),
            Some(MatchRank {
                tier: 0,
                sub_rank: 0
            })
        );
        assert!(score_primary_candidate(&q, 1, "foo", None).is_none());
    }
}
