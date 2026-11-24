use super::{parse_workshop_id_col, serialize_json_enum, ServerRepo};
use crate::search::{self, MatchRank, ParsedQuery};
use l4a_protocol::{MetadataSource, SearchQuery, SearchResponse, WorkshopId};
use rusqlite::{params, Connection, Row, Rows};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

const SEARCH_ITEM_FILTERS: &str = "
    (?1 IS NULL OR EXISTS (SELECT 1 FROM json_each(w.tags_json) WHERE LOWER(value) = ?1))
    AND (?2 IS NULL OR w.status = ?2)
    AND (?3 = 0 OR (w.catalog_indexed = 1 AND w.is_listed = 1))
    AND (?4 IS NULL OR w.file_type = ?4)";

const SEARCH_ORDER_COLUMNS: &str = "
    w.workshop_id, w.title, COALESCE(w.subscriptions, 0),
    COALESCE(w.time_updated, 0), COALESCE(w.time_created, 0),
    w.status = 'active', w.is_admin_override";
const SEARCH_FTS_SOURCE: &str = "
    FROM workshop_items_fts JOIN workshop_items w ON w.workshop_id = workshop_items_fts.rowid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RankedCatalogMatch {
    pub id: WorkshopId,
    pub rank: Option<MatchRank>,
    pub is_active: bool,
    pub is_admin_override: bool,
    pub sort_value: i64,
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
    file_type: Option<String>,
}

impl SearchFilters {
    fn new(query: &SearchQuery, public_only: bool) -> Self {
        Self {
            tag: query
                .tag
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase),
            status: query.status.map(|status| serialize_json_enum(&status)),
            public_only,
            file_type: query.file_type.map(|ft| serialize_json_enum(&ft)),
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

struct SearchPage {
    sort: SearchSort,
    offset: u64,
    limit: u64,
    total: u64,
    best: BinaryHeap<RankedEntry>,
}

impl SearchPage {
    fn new(sort: SearchSort, page_size: Option<u32>, offset: u64) -> Self {
        Self {
            sort,
            offset,
            limit: page_size.map_or(u64::MAX, |size| offset + u64::from(size)),
            total: 0,
            best: BinaryHeap::new(),
        }
    }

    fn collect(
        &mut self,
        mut rows: Rows<'_>,
        score: impl Fn(&Row<'_>, WorkshopId) -> rusqlite::Result<Option<MatchRank>>,
    ) -> rusqlite::Result<()> {
        while let Some(row) = rows.next()? {
            let id = parse_workshop_id_col(0, row.get(0)?)?;
            if let Some(rank) = score(row, id)? {
                self.total += 1;
                let entry = RankedEntry {
                    entry: self.sort.entry(row, id, rank)?,
                    matched: self.sort.ranked_match(row, id, Some(rank))?,
                };
                // Count every validated match and keep only the best entries through this page.
                if (self.best.len() as u64) < self.limit {
                    self.best.push(entry);
                } else if let Some(mut last) = self.best.peek_mut() {
                    if entry.entry < last.entry {
                        *last = entry;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> RankedMatchPage {
        let offset = usize::try_from(self.offset).unwrap_or(usize::MAX);
        let page_len = self.best.len().saturating_sub(offset);
        // The requested page is at the end of the retained entries in sort order.
        let mut matches: Vec<_> = std::iter::from_fn(|| self.best.pop())
            .take(page_len)
            .map(|ranked| ranked.matched)
            .collect();
        matches.reverse();
        (self.total, matches)
    }
}

impl ServerRepo {
    pub fn search(&self, query: &SearchQuery, public_only: bool) -> anyhow::Result<SearchResponse> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let offset = u64::from(page - 1) * u64::from(page_size);
        self.with_read_snapshot(|conn| {
            let (total, matches) =
                Self::ranked_matches(conn, query, public_only, offset, Some(page_size))?;
            let mut items = Vec::with_capacity(matches.len());
            for matched in matches {
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
    pub(crate) fn catalog_search_matches(
        &self,
        query: &SearchQuery,
    ) -> anyhow::Result<Vec<RankedCatalogMatch>> {
        self.with_read_snapshot(|conn| {
            let (_, matches) = Self::ranked_matches(conn, query, true, 0, None)?;
            Ok(matches)
        })
    }

    fn ranked_matches(
        conn: &Connection,
        query: &SearchQuery,
        public_only: bool,
        offset: u64,
        limit: Option<u32>,
    ) -> anyhow::Result<RankedMatchPage> {
        let filters = SearchFilters::new(query, public_only);
        let sort = SearchSort::from_name(query.sort.as_deref());
        let target = LocalSearchTarget::from_name(query.search_target.as_deref());
        let parsed = query.query.as_deref().and_then(search::parse_query);

        let Some(parsed) = parsed else {
            return Self::browse_items(conn, &filters, sort, limit, offset);
        };
        if let Some(id) = parsed
            .exact_workshop_id
            .filter(|_| parsed.numeric_prefix.is_none())
        {
            return Self::search_exact_id(conn, &filters, sort, id, offset);
        }

        let mut selection = SearchPage::new(sort, limit, offset);
        if parsed.terms.is_empty() {
            Self::search_symbol_only(conn, &parsed, &filters, target, &mut selection)?;
        } else {
            Self::search_indexed(conn, &parsed, &filters, target, &mut selection)?;
        }
        Ok(selection.finish())
    }

    fn browse_items(
        conn: &Connection,
        filters: &SearchFilters,
        sort: SearchSort,
        page_size: Option<u32>,
        offset: u64,
    ) -> anyhow::Result<RankedMatchPage> {
        let order_by = sort.browse_order();
        let sql = format!(
            "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}
             ORDER BY {order_by} LIMIT ?5 OFFSET ?6"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(
            params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type,
                page_size.map_or(-1, i64::from),
                offset
            ],
            |row| sort.ranked_match(row, parse_workshop_id_col(0, row.get(0)?)?, None),
        )?;
        let matches = rows.collect::<Result<Vec<_>, _>>()?;
        let total = if page_size.is_some() {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}"),
                params![
                    filters.tag,
                    filters.status,
                    filters.public_only,
                    filters.file_type
                ],
                |r| r.get(0),
            )?
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
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w
             WHERE w.workshop_id = ?5 AND {SEARCH_ITEM_FILTERS}"
        ))?;
        let mut rows = stmt.query(params![
            filters.tag,
            filters.status,
            filters.public_only,
            filters.file_type,
            id
        ])?;
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
    ) -> anyhow::Result<()> {
        if target != LocalSearchTarget::DescriptionOnly {
            if let Some(fts_query) = search::build_primary_fts_query(parsed) {
                let (source, match_filter) = if parsed.numeric_prefix.is_some() {
                    (
                        "FROM (
                             SELECT rowid AS workshop_id FROM workshop_items_fts WHERE workshop_items_fts MATCH ?5
                             UNION
                             SELECT workshop_id FROM workshop_items
                             WHERE workshop_id = ?6 OR CAST(workshop_id AS TEXT) LIKE ?7
                         ) candidates JOIN workshop_items w ON w.workshop_id = candidates.workshop_id",
                        "",
                    )
                } else {
                    (SEARCH_FTS_SOURCE, "AND workshop_items_fts MATCH ?5")
                };
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {SEARCH_ORDER_COLUMNS}, w.creator_name
                     {source} WHERE {SEARCH_ITEM_FILTERS} {match_filter}"
                ))?;
                let rows = if let Some(prefix) = &parsed.numeric_prefix {
                    let exact = parsed
                        .exact_workshop_id
                        .and_then(|id| i64::try_from(id).ok());
                    stmt.query(params![
                        filters.tag,
                        filters.status,
                        filters.public_only,
                        filters.file_type,
                        fts_query,
                        exact,
                        format!("{prefix}%")
                    ])?
                } else {
                    stmt.query(params![
                        filters.tag,
                        filters.status,
                        filters.public_only,
                        filters.file_type,
                        fts_query
                    ])?
                };
                page.collect(rows, |row, id| {
                    let title = row.get_ref(1)?.as_str()?;
                    let creator = row.get_ref(7)?.as_str_or_null()?;
                    Ok(match target {
                        LocalSearchTarget::TitleOnly => {
                            search::score_title_only_candidate(parsed, id.as_u64(), title)
                        }
                        LocalSearchTarget::All => {
                            search::score_primary_candidate(parsed, id.as_u64(), title, creator)
                        }
                        LocalSearchTarget::DescriptionOnly => unreachable!(),
                    })
                })?;
            }
        }

        if target == LocalSearchTarget::DescriptionOnly
            || (target == LocalSearchTarget::All && page.total == 0)
        {
            if let Some(desc_query) = search::build_desc_fts_query(parsed) {
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {SEARCH_ORDER_COLUMNS}, w.description {SEARCH_FTS_SOURCE}
                     WHERE workshop_items_fts MATCH ?5 AND {SEARCH_ITEM_FILTERS}"
                ))?;
                let rows = stmt.query(params![
                    filters.tag,
                    filters.status,
                    filters.public_only,
                    filters.file_type,
                    desc_query
                ])?;
                page.collect(rows, |row, _| {
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
    ) -> anyhow::Result<()> {
        if target != LocalSearchTarget::DescriptionOnly {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}"
            ))?;
            let rows = stmt.query(params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type
            ])?;
            page.collect(rows, |row, _| {
                let title = search::fold_text(row.get_ref(1)?.as_str()?);
                let rank = if title == parsed.raw_folded {
                    Some(MatchRank {
                        tier: 0,
                        sub_rank: 1,
                    })
                } else if title.starts_with(&parsed.raw_folded) {
                    Some(MatchRank {
                        tier: 1,
                        sub_rank: 0,
                    })
                } else if title.contains(&parsed.raw_folded) {
                    Some(MatchRank {
                        tier: 2,
                        sub_rank: 0,
                    })
                } else {
                    None
                };
                Ok(rank)
            })?;
        }

        if target == LocalSearchTarget::DescriptionOnly
            || (target == LocalSearchTarget::All && page.total == 0)
        {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS}, w.description FROM workshop_items w
                 WHERE {SEARCH_ITEM_FILTERS} AND w.description IS NOT NULL"
            ))?;
            let rows = stmt.query(params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type
            ])?;
            page.collect(rows, |row, _| {
                let description = row.get_ref(7)?.as_str()?;
                let cleaned = search::strip_bbcode_and_urls(description);
                let matches = search::fold_text(&cleaned).contains(&parsed.raw_folded);
                Ok(matches.then_some(MatchRank {
                    tier: 6,
                    sub_rank: 0,
                }))
            })?;
        }

        Ok(())
    }
}
