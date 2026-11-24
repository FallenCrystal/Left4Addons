mod pinyin;
mod query;
mod scoring;
#[cfg(test)]
mod tests;
mod text;

pub use query::{build_desc_fts_query, build_primary_fts_query, parse_query};
pub use scoring::{score_desc_candidate, score_primary_candidate, score_title_only_candidate};
pub use text::{fold_text, strip_bbcode_and_urls, tokenize_text};

use pinyin::build_pinyin_tokens;
use text::{build_compact_tokens, tokenize_with_aliases, tokens_to_fts_string};

pub const SEARCH_INDEX_VERSION: &str = "2";

const MAX_COMPACT_SPAN: usize = 4;
const MAX_COMPACT_LEN: usize = 40;
const MAX_PINYIN_SPAN: usize = 6;
const PINYIN_PAIR_KEY_LEN: usize = 8;
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
    pub desc_aliases: String,
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
    compact_all: String,
}

impl ParsedQuery {
    fn contains_required_literals(&self, folded: &str) -> bool {
        self.literal_substrings.iter().all(|s| folded.contains(s))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchRank {
    pub tier: u8,
    pub sub_rank: u8,
}

pub fn build_indexed_document(
    title: &str,
    creator_name: Option<&str>,
    description: Option<&str>,
) -> IndexedDocument {
    let title_toks = tokenize_with_aliases(title);
    let title_tokens = tokens_to_fts_string(&title_toks.tokens);
    let title_compact = build_compact_tokens(&title_toks, MAX_COMPACT_SPAN);
    let title_pinyin = build_pinyin_tokens(title);

    let creator_tokens = creator_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|creator| {
            let toks = tokenize_with_aliases(creator);
            let base = tokens_to_fts_string(&toks.tokens);
            let compact = build_compact_tokens(&toks, 3);
            if compact.is_empty() {
                base
            } else if base.is_empty() {
                compact
            } else {
                format!("{base} {compact}")
            }
        })
        .unwrap_or_default();

    let desc = description
        .map(strip_bbcode_and_urls)
        .map(|cleaned| tokenize_with_aliases(&cleaned))
        .unwrap_or_default();

    IndexedDocument {
        title_tokens,
        title_compact,
        title_pinyin,
        creator_tokens,
        desc_tokens: tokens_to_fts_string(&desc.tokens),
        desc_aliases: desc
            .unsplit_words
            .iter()
            .map(|(_, word)| word.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    }
}
