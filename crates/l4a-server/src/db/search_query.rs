use super::{
    parse_workshop_id_col, serialize_json_enum, QueryAbortedError, QueryCancelToken, ServerRepo,
};
use crate::search::{self, MatchRank, ParsedQuery};
use l4a_protocol::{MetadataSource, SearchQuery, SearchResponse, WorkshopFileType, WorkshopId};
use rusqlite::{params_from_iter, types::Value, Connection, Row, Rows};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

const SEARCH_ORDER_COLUMNS: &str = "
    w.workshop_id, w.title, COALESCE(w.subscriptions, 0),
    COALESCE(w.time_updated, 0), COALESCE(w.time_created, 0),
    w.status = 'active', w.is_admin_override";
// Keep FTS5 on the outside of the join so SQLite runs MATCH once before looking up items.
const FTS_FIRST_SOURCE: &str = "
    FROM workshop_items_fts CROSS JOIN workshop_items w ON w.workshop_id = workshop_items_fts.rowid";
const FTS_COLLECTION_SOURCE: &str = "
    FROM workshop_items_fts JOIN workshop_items w ON w.workshop_id = workshop_items_fts.rowid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RankedCatalogMatch {
    pub id: WorkshopId,
    pub rank: Option<MatchRank>,
    pub is_active: bool,
    pub is_admin_override: bool,
    pub sort_value: i64,
    pub secondary_sort_value: i64,
}

type RankedMatchPage = (u64, Vec<RankedCatalogMatch>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalSearchTarget {
    All,
    TitleOnly,
    DescriptionOnly,
}

impl LocalSearchTarget {
    fn from_name(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("title" | "1") => Self::TitleOnly,
            Some("description" | "desc" | "2") => Self::DescriptionOnly,
            _ => Self::All,
        }
    }
}

struct SearchFilters {
    tag: Option<String>,
    status: Option<String>,
    public_only: bool,
    file_type: Option<WorkshopFileType>,
    exact_untyped_id: Option<i64>,
}

impl SearchFilters {
    fn new(query: &SearchQuery, public_only: bool, exact_untyped_id: Option<WorkshopId>) -> Self {
        Self {
            tag: query
                .tag
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase),
            status: query.status.map(|status| serialize_json_enum(&status)),
            public_only,
            file_type: query.file_type,
            exact_untyped_id: exact_untyped_id.and_then(|wid| i64::try_from(wid.as_u64()).ok()),
        }
    }

    fn fts_source(&self) -> &'static str {
        if self.file_type == Some(WorkshopFileType::Collection) {
            // For selective collection filters, let SQLite look up catalog rows first.
            FTS_COLLECTION_SOURCE
        } else {
            FTS_FIRST_SOURCE
        }
    }

    /// Build an index-friendly WHERE predicate list without optional-parameter OR disjunctions.
    fn build_where(&self, params: &mut Vec<Value>) -> String {
        let mut clauses = Vec::new();
        if self.public_only {
            clauses.push("w.catalog_indexed = 1 AND w.is_listed = 1".to_string());
        }
        if let Some(ft) = self.file_type {
            params.push(Value::Text(serialize_json_enum(&ft)));
            let ft_idx = params.len();
            if let Some(exact_id) = self.exact_untyped_id {
                params.push(Value::Integer(exact_id));
                let id_idx = params.len();
                clauses.push(format!(
                    "(w.file_type = ?{ft_idx} OR w.workshop_id = ?{id_idx})"
                ));
            } else {
                clauses.push(format!("w.file_type = ?{ft_idx}"));
            }
        }
        if let Some(ref status) = self.status {
            params.push(Value::Text(status.clone()));
            clauses.push(format!("w.status = ?{}", params.len()));
        }
        if let Some(ref tag) = self.tag {
            params.push(Value::Text(tag.clone()));
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM json_each(w.tags_json) WHERE LOWER(value) = ?{})",
                params.len()
            ));
        }
        if clauses.is_empty() {
            "1 = 1".to_string()
        } else {
            clauses.join(" AND ")
        }
    }
}

#[derive(Clone, Copy)]
enum SearchSort {
    Updated,
    UpdatedAscending,
    Title,
    Subscriptions,
    Newest,
    Relevance,
}

impl SearchSort {
    fn from_name(sort: Option<&str>) -> Self {
        match sort.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("updated" | "lastupdated") => Self::Updated,
            Some("updated_asc") => Self::UpdatedAscending,
            Some("title") => Self::Title,
            Some("subscriptions" | "totalprofiles" | "popular" | "trend" | "toprated") => {
                Self::Subscriptions
            }
            Some("newest" | "mostrecent") => Self::Newest,
            _ => Self::Relevance,
        }
    }

    fn browse_order(self) -> &'static str {
        match self {
            Self::Updated => "COALESCE(w.time_updated, 0) DESC, w.workshop_id DESC",
            Self::UpdatedAscending => "COALESCE(w.time_updated, 0) ASC, w.workshop_id ASC",
            Self::Title => "w.title COLLATE NOCASE ASC, w.workshop_id ASC",
            Self::Subscriptions => "COALESCE(w.subscriptions, 0) DESC, w.workshop_id ASC",
            Self::Newest => "COALESCE(w.time_created, 0) DESC, w.workshop_id DESC",
            Self::Relevance => {
                "COALESCE(w.subscriptions, 0) DESC, COALESCE(w.time_updated, 0) DESC, w.workshop_id ASC"
            }
        }
    }

    fn entry(self, row: &Row<'_>, id: WorkshopId, rank: MatchRank) -> rusqlite::Result<PageEntry> {
        Ok(match self {
            Self::Updated => PageEntry::TimeDescending(Reverse((row.get(3)?, id))),
            Self::UpdatedAscending => PageEntry::TimeAscending(row.get(3)?, id),
            Self::Title => PageEntry::Title(row.get_ref(1)?.as_str()?.to_ascii_lowercase(), id),
            Self::Subscriptions => PageEntry::Subscriptions(Reverse(row.get(2)?), id),
            Self::Newest => PageEntry::TimeDescending(Reverse((row.get(4)?, id))),
            Self::Relevance => {
                PageEntry::Relevance(rank, Reverse(row.get(2)?), Reverse(row.get(3)?), id)
            }
        })
    }

    fn ranked_match(
        self,
        row: &Row<'_>,
        id: WorkshopId,
        rank: Option<MatchRank>,
    ) -> rusqlite::Result<RankedCatalogMatch> {
        Ok(RankedCatalogMatch {
            id,
            rank,
            is_active: row.get(5)?,
            is_admin_override: row.get(6)?,
            sort_value: match self {
                Self::Subscriptions | Self::Relevance => row.get(2)?,
                Self::Updated => row.get(3)?,
                Self::Newest => row.get(4)?,
                _ => 0,
            },
            secondary_sort_value: row.get(3)?,
        })
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PageEntry {
    TimeDescending(Reverse<(i64, WorkshopId)>),
    TimeAscending(i64, WorkshopId),
    Title(String, WorkshopId),
    Subscriptions(Reverse<i64>, WorkshopId),
    Relevance(MatchRank, Reverse<i64>, Reverse<i64>, WorkshopId),
}

#[derive(PartialEq, Eq)]
struct RankedEntry {
    entry: PageEntry,
    matched: RankedCatalogMatch,
}

impl Ord for RankedEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.entry.cmp(&other.entry)
    }
}

impl PartialOrd for RankedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

enum PageStorage {
    Bounded {
        limit: u64,
        best: BinaryHeap<RankedEntry>,
    },
    Unbounded(Vec<RankedEntry>),
}

struct SearchPage {
    sort: SearchSort,
    offset: u64,
    total: u64,
    storage: PageStorage,
}

impl SearchPage {
    fn new(sort: SearchSort, page_size: Option<u32>, offset: u64) -> Self {
        let storage = match page_size {
            Some(size) => PageStorage::Bounded {
                limit: offset.saturating_add(u64::from(size)),
                best: BinaryHeap::new(),
            },
            None => PageStorage::Unbounded(Vec::new()),
        };
        Self {
            sort,
            offset,
            total: 0,
            storage,
        }
    }

    fn collect(
        &mut self,
        mut rows: Rows<'_>,
        cancel: &QueryCancelToken,
        score: impl Fn(&Row<'_>, WorkshopId) -> rusqlite::Result<Option<MatchRank>>,
    ) -> anyhow::Result<()> {
        let mut scanned: u32 = 0;
        while let Some(row) = rows.next()? {
            scanned = scanned.wrapping_add(1);
            if scanned & 0xFF == 0 && cancel.is_cancelled() {
                return Err(QueryAbortedError::Cancelled.into());
            }
            let id = parse_workshop_id_col(0, row.get(0)?)?;
            if let Some(rank) = score(row, id)? {
                self.total += 1;
                let entry = RankedEntry {
                    entry: self.sort.entry(row, id, rank)?,
                    matched: self.sort.ranked_match(row, id, Some(rank))?,
                };
                match &mut self.storage {
                    PageStorage::Bounded { limit, best } => {
                        // Count every validated match and keep only the best entries through this page.
                        if (best.len() as u64) < *limit {
                            best.push(entry);
                        } else if let Some(mut last) = best.peek_mut() {
                            if entry.entry < last.entry {
                                *last = entry;
                            }
                        }
                    }
                    PageStorage::Unbounded(all) => {
                        all.push(entry);
                    }
                }
            }
        }
        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }
        Ok(())
    }

    fn finish(self) -> RankedMatchPage {
        let offset = usize::try_from(self.offset).unwrap_or(usize::MAX);
        match self.storage {
            PageStorage::Bounded { mut best, .. } => {
                let page_len = best.len().saturating_sub(offset);
                // The requested page is at the end of the retained entries in sort order.
                let mut matches: Vec<_> = std::iter::from_fn(|| best.pop())
                    .take(page_len)
                    .map(|ranked| ranked.matched)
                    .collect();
                matches.reverse();
                (self.total, matches)
            }
            PageStorage::Unbounded(mut all) => {
                all.sort_unstable();
                let matches = all
                    .into_iter()
                    .skip(offset)
                    .map(|ranked| ranked.matched)
                    .collect();
                (self.total, matches)
            }
        }
    }
}

/// Convert a decimal prefix (e.g. `"610"`) into disjoint `INTEGER PRIMARY KEY` B-tree ranges
/// matching all positive `i64` values whose decimal representation starts with `prefix`.
fn numeric_prefix_ranges(prefix: &str) -> Vec<(i64, i64)> {
    if prefix.is_empty() || prefix.starts_with('0') || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return Vec::new();
    }
    let Ok(base) = prefix.parse::<u128>() else {
        return Vec::new();
    };
    let max_i64 = i64::MAX as u128;
    if base == 0 || base > max_i64 {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut scale: u128 = 1;
    while let Some(lo) = base.checked_mul(scale) {
        if lo > max_i64 {
            break;
        }
        let hi = lo.saturating_add(scale - 1).min(max_i64);
        ranges.push((lo as i64, hi as i64));
        let Some(next_scale) = scale.checked_mul(10) else {
            break;
        };
        scale = next_scale;
    }
    ranges
}

impl ServerRepo {
    pub fn search(&self, query: &SearchQuery, public_only: bool) -> anyhow::Result<SearchResponse> {
        self.search_cancellable(query, public_only, &QueryCancelToken::new())
    }

    pub fn search_cancellable(
        &self,
        query: &SearchQuery,
        public_only: bool,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<SearchResponse> {
        self.search_cancellable_with_exact_untyped(query, public_only, None, cancel)
    }

    pub(crate) fn search_cancellable_with_exact_untyped(
        &self,
        query: &SearchQuery,
        public_only: bool,
        exact_untyped_id: Option<WorkshopId>,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<SearchResponse> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let offset = u64::from(page - 1) * u64::from(page_size);
        self.with_cancellable_read_snapshot(cancel, |conn| {
            let (total, matches) = Self::ranked_matches(
                conn,
                query,
                public_only,
                exact_untyped_id,
                offset,
                Some(page_size),
                cancel,
            )?;
            let mut items = Vec::with_capacity(matches.len());
            for matched in matches {
                if cancel.is_cancelled() {
                    return Err(QueryAbortedError::Cancelled.into());
                }
                if let Some(meta) = Self::get_item_with_conn(conn, matched.id)? {
                    items.push(meta.record);
                }
            }
            Ok(SearchResponse {
                items,
                total,
                page,
                page_size,
                source: MetadataSource::CatalogServer,
            })
        })
    }

    /// Return compact matches for the hybrid merge, which loads full local records after paging.
    pub(crate) fn catalog_search_matches_with_exact_untyped(
        &self,
        query: &SearchQuery,
        exact_untyped_id: Option<WorkshopId>,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<Vec<RankedCatalogMatch>> {
        self.with_cancellable_read_snapshot(cancel, |conn| {
            let (_, matches) =
                Self::ranked_matches(conn, query, true, exact_untyped_id, 0, None, cancel)?;
            Ok(matches)
        })
    }

    fn ranked_matches(
        conn: &Connection,
        query: &SearchQuery,
        public_only: bool,
        exact_untyped_id: Option<WorkshopId>,
        offset: u64,
        limit: Option<u32>,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<RankedMatchPage> {
        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }
        let filters = SearchFilters::new(query, public_only, exact_untyped_id);
        let sort = SearchSort::from_name(query.sort.as_deref());
        let target = LocalSearchTarget::from_name(query.search_target.as_deref());
        let parsed = query.query.as_deref().and_then(search::parse_query);

        let Some(parsed) = parsed else {
            return Self::browse_items(conn, &filters, sort, limit, offset, cancel);
        };
        if let Some(id) = parsed
            .exact_workshop_id
            .filter(|_| parsed.numeric_prefix.is_none())
        {
            return Self::search_exact_id(conn, &filters, sort, id, offset);
        }

        let mut selection = SearchPage::new(sort, limit, offset);
        if parsed.terms.is_empty() {
            Self::search_symbol_only(conn, &parsed, &filters, target, &mut selection, cancel)?;
        } else {
            Self::search_indexed(conn, &parsed, &filters, target, &mut selection, cancel)?;
        }
        Ok(selection.finish())
    }

    fn browse_items(
        conn: &Connection,
        filters: &SearchFilters,
        sort: SearchSort,
        page_size: Option<u32>,
        offset: u64,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<RankedMatchPage> {
        let mut filter_params = Vec::new();
        let where_clause = filters.build_where(&mut filter_params);
        let order_by = sort.browse_order();

        let mut list_params = filter_params.clone();
        list_params.push(Value::Integer(page_size.map_or(-1, i64::from)));
        let limit_idx = list_params.len();
        list_params.push(Value::Integer(i64::try_from(offset).unwrap_or(i64::MAX)));
        let offset_idx = list_params.len();

        let sql = format!(
            "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w WHERE {where_clause}
             ORDER BY {order_by} LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let mut rows = stmt.query(params_from_iter(list_params.iter()))?;
        let mut matches = Vec::new();
        let mut scanned: u32 = 0;
        while let Some(row) = rows.next()? {
            scanned = scanned.wrapping_add(1);
            if scanned & 0xFF == 0 && cancel.is_cancelled() {
                return Err(QueryAbortedError::Cancelled.into());
            }
            matches.push(sort.ranked_match(row, parse_workshop_id_col(0, row.get(0)?)?, None)?);
        }
        drop(rows);
        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }
        let total = if page_size.is_some() {
            let count_sql = format!("SELECT COUNT(*) FROM workshop_items w WHERE {where_clause}");
            let mut count_stmt = conn.prepare_cached(&count_sql)?;
            count_stmt.query_row(params_from_iter(filter_params.iter()), |r| r.get(0))?
        } else {
            matches.len() as u64
        };
        Ok((total, matches))
    }

    fn search_exact_id(
        conn: &Connection,
        filters: &SearchFilters,
        sort: SearchSort,
        id: u64,
        offset: u64,
    ) -> anyhow::Result<RankedMatchPage> {
        let Ok(id) = i64::try_from(id) else {
            return Ok((0, Vec::new()));
        };
        let mut params = vec![Value::Integer(id)];
        let where_clause = filters.build_where(&mut params);
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w
             WHERE w.workshop_id = ?1 AND {where_clause}"
        ))?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        let Some(row) = rows.next()? else {
            return Ok((0, Vec::new()));
        };
        let matches = if offset == 0 {
            let rank = MatchRank {
                tier: 0,
                sub_rank: 0,
            };
            vec![sort.ranked_match(row, WorkshopId(id as u64), Some(rank))?]
        } else {
            Vec::new()
        };
        Ok((1, matches))
    }

    fn search_indexed(
        conn: &Connection,
        parsed: &ParsedQuery,
        filters: &SearchFilters,
        target: LocalSearchTarget,
        page: &mut SearchPage,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<()> {
        let fts_source = filters.fts_source();
        if target != LocalSearchTarget::DescriptionOnly {
            let fts_query = match target {
                LocalSearchTarget::TitleOnly => search::build_title_only_fts_query(parsed),
                LocalSearchTarget::All => search::build_primary_fts_query(parsed),
                LocalSearchTarget::DescriptionOnly => unreachable!(),
            };
            if let Some(fts_query) = fts_query {
                let mut params = Vec::new();
                let where_clause = filters.build_where(&mut params);
                params.push(Value::Text(fts_query));
                let fts_idx = params.len();

                let mut numeric_ranges = if target == LocalSearchTarget::All {
                    parsed
                        .numeric_prefix
                        .as_deref()
                        .map(numeric_prefix_ranges)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if target == LocalSearchTarget::All && numeric_ranges.is_empty() {
                    if let Some(exact) = parsed
                        .exact_workshop_id
                        .and_then(|id| i64::try_from(id).ok())
                    {
                        numeric_ranges.push((exact, exact));
                    }
                }

                let sql = if !numeric_ranges.is_empty() {
                    let mut union_parts = vec![format!(
                        "SELECT rowid AS workshop_id FROM workshop_items_fts WHERE workshop_items_fts MATCH ?{fts_idx}"
                    )];
                    for (lo, hi) in numeric_ranges {
                        params.push(Value::Integer(lo));
                        let lo_idx = params.len();
                        params.push(Value::Integer(hi));
                        let hi_idx = params.len();
                        union_parts.push(format!(
                            "SELECT workshop_id FROM workshop_items WHERE workshop_id BETWEEN ?{lo_idx} AND ?{hi_idx}"
                        ));
                    }
                    let candidates = union_parts.join(" UNION ");
                    format!(
                        "SELECT {SEARCH_ORDER_COLUMNS}, w.creator_name
                         FROM ({candidates}) candidates
                         JOIN workshop_items w ON w.workshop_id = candidates.workshop_id
                         WHERE {where_clause}"
                    )
                } else {
                    format!(
                        "SELECT {SEARCH_ORDER_COLUMNS}, w.creator_name
                         {fts_source}
                         WHERE workshop_items_fts MATCH ?{fts_idx} AND {where_clause}"
                    )
                };

                let mut stmt = conn.prepare_cached(&sql)?;
                let rows = stmt.query(params_from_iter(params.iter()))?;
                page.collect(rows, cancel, |row, id| {
                    let title = row.get_ref(1)?.as_str()?;
                    let creator = row.get_ref(7)?.as_str_or_null()?;
                    Ok(match target {
                        LocalSearchTarget::TitleOnly => {
                            search::score_title_only_candidate(parsed, title)
                        }
                        LocalSearchTarget::All => {
                            search::score_primary_candidate(parsed, id.as_u64(), title, creator)
                        }
                        LocalSearchTarget::DescriptionOnly => unreachable!(),
                    })
                })?;
            }
        }

        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }

        if target == LocalSearchTarget::DescriptionOnly
            || (target == LocalSearchTarget::All && page.total == 0)
        {
            if let Some(desc_query) = search::build_desc_fts_query(parsed) {
                let mut params = Vec::new();
                let where_clause = filters.build_where(&mut params);
                params.push(Value::Text(desc_query));
                let fts_idx = params.len();
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {SEARCH_ORDER_COLUMNS}, w.description {fts_source}
                     WHERE workshop_items_fts MATCH ?{fts_idx} AND {where_clause}"
                ))?;
                let rows = stmt.query(params_from_iter(params.iter()))?;
                page.collect(rows, cancel, |row, _| {
                    let description = row.get_ref(7)?.as_str_or_null()?;
                    Ok(description.and_then(|desc| search::score_desc_candidate(parsed, desc)))
                })?;
            }
        }

        Ok(())
    }

    fn search_symbol_only(
        conn: &Connection,
        parsed: &ParsedQuery,
        filters: &SearchFilters,
        target: LocalSearchTarget,
        page: &mut SearchPage,
        cancel: &QueryCancelToken,
    ) -> anyhow::Result<()> {
        if target != LocalSearchTarget::DescriptionOnly {
            let mut params = Vec::new();
            let where_clause = filters.build_where(&mut params);
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w WHERE {where_clause}"
            ))?;
            let rows = stmt.query(params_from_iter(params.iter()))?;
            page.collect(rows, cancel, |row, _| {
                let title = row.get_ref(1)?.as_str()?;
                Ok(search::score_symbol_only_title(parsed, title))
            })?;
        }

        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }

        if target == LocalSearchTarget::DescriptionOnly
            || (target == LocalSearchTarget::All && page.total == 0)
        {
            let mut params = Vec::new();
            let where_clause = filters.build_where(&mut params);
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS}, w.description FROM workshop_items w
                 WHERE {where_clause} AND w.description IS NOT NULL"
            ))?;
            let rows = stmt.query(params_from_iter(params.iter()))?;
            page.collect(rows, cancel, |row, _| {
                let description = row.get_ref(7)?.as_str()?;
                Ok(search::score_symbol_only_desc(parsed, description))
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_text_search_uses_an_index_suited_to_the_file_type() {
        let repo = ServerRepo::open_in_memory().unwrap();
        for (file_type, expected_index) in [
            (WorkshopFileType::Item, "workshop_items_fts"),
            (WorkshopFileType::Collection, "file_type=?"),
        ] {
            let query = SearchQuery {
                file_type: Some(file_type),
                ..SearchQuery::default()
            };
            let filters = SearchFilters::new(&query, true, None);
            let mut params = Vec::new();
            let where_clause = filters.build_where(&mut params);
            params.push(Value::Text(
                "{title_tokens creator_tokens} : \"增 城\"".into(),
            ));
            let fts_source = filters.fts_source();
            let sql = format!(
                "EXPLAIN QUERY PLAN
                 SELECT {SEARCH_ORDER_COLUMNS}, w.creator_name {fts_source}
                 WHERE workshop_items_fts MATCH ?{} AND {where_clause}",
                params.len()
            );
            let first_step = repo
                .with_read_snapshot(|conn| {
                    Ok(conn.query_row(&sql, params_from_iter(&params), |row| {
                        row.get::<_, String>(3)
                    })?)
                })
                .unwrap();

            assert!(
                first_step.contains(expected_index),
                "{file_type:?}: {first_step}"
            );
        }
    }
}
