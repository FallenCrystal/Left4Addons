use super::{parse_workshop_id_col, serialize_json_enum, ServerRepo};
use crate::search::{self, MatchRank, ParsedQuery};
use l4a_protocol::{MetadataSource, SearchQuery, SearchResponse, WorkshopId};
use rusqlite::{params, Connection, Row, Rows};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

const SEARCH_ITEM_FILTERS: &str = "
    (?1 IS NULL OR EXISTS (SELECT 1 FROM json_each(w.tags_json) WHERE LOWER(value) = ?1))
    AND (?2 IS NULL OR w.status = ?2)
    AND (?3 = 0 OR (w.catalog_indexed = 1 AND w.is_listed = 1))";

const SEARCH_ORDER_COLUMNS: &str = "
    w.workshop_id, w.title, COALESCE(w.subscriptions, 0),
    COALESCE(w.time_updated, 0), COALESCE(w.time_created, 0)";
const SEARCH_FTS_SOURCE: &str = "
    FROM workshop_items_fts JOIN workshop_items w ON w.workshop_id = workshop_items_fts.rowid";

struct SearchFilters {
    tag: Option<String>,
    status: Option<String>,
    public_only: bool,
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
        match sort {
            Some("updated") => Self::Updated,
            Some("updated_asc") => Self::UpdatedAscending,
            Some("title") => Self::Title,
            Some("subscriptions") => Self::Subscriptions,
            Some("newest") => Self::Newest,
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
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PageEntry {
    TimeDescending(Reverse<(i64, WorkshopId)>),
    TimeAscending(i64, WorkshopId),
    Title(String, WorkshopId),
    Subscriptions(Reverse<i64>, WorkshopId),
    Relevance(MatchRank, Reverse<i64>, Reverse<i64>, WorkshopId),
}

impl PageEntry {
    fn id(self) -> WorkshopId {
        match self {
            Self::TimeDescending(Reverse((_, id)))
            | Self::TimeAscending(_, id)
            | Self::Title(_, id)
            | Self::Subscriptions(_, id)
            | Self::Relevance(_, _, _, id) => id,
        }
    }
}

struct SearchPage {
    sort: SearchSort,
    offset: u64,
    limit: u64,
    total: u64,
    best: BinaryHeap<PageEntry>,
}

impl SearchPage {
    fn new(sort: SearchSort, page_size: u32, offset: u64) -> Self {
        Self {
            sort,
            offset,
            limit: offset + u64::from(page_size),
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
                let entry = self.sort.entry(row, id, rank)?;
                // Count every validated match and keep only the best entries through this page.
                if (self.best.len() as u64) < self.limit {
                    self.best.push(entry);
                } else if let Some(mut last) = self.best.peek_mut() {
                    if entry < *last {
                        *last = entry;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> (u64, Vec<WorkshopId>) {
        let offset = usize::try_from(self.offset).unwrap_or(usize::MAX);
        let page_len = self.best.len().saturating_sub(offset);
        // The requested page is at the end of the retained entries in sort order.
        let mut ids: Vec<_> = std::iter::from_fn(|| self.best.pop())
            .take(page_len)
            .map(PageEntry::id)
            .collect();
        ids.reverse();
        (self.total, ids)
    }
}

impl ServerRepo {
    pub fn search(&self, query: &SearchQuery, public_only: bool) -> anyhow::Result<SearchResponse> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let offset = u64::from(page - 1) * u64::from(page_size);
        let filters = SearchFilters {
            tag: query
                .tag
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase),
            status: query.status.map(|status| serialize_json_enum(&status)),
            public_only,
        };
        let sort = SearchSort::from_name(query.sort.as_deref());
        let parsed = query.query.as_deref().and_then(search::parse_query);

        self.with_read_snapshot(|conn| {
            let (total, page_ids) = match &parsed {
                None => Self::browse_items(conn, &filters, sort, page_size, offset)?,
                Some(parsed) => {
                    if let Some(id) = parsed
                        .exact_workshop_id
                        .filter(|_| parsed.numeric_prefix.is_none())
                    {
                        Self::search_exact_id(conn, &filters, id, offset)?
                    } else {
                        let mut selection = SearchPage::new(sort, page_size, offset);
                        if parsed.terms.is_empty() {
                            Self::search_symbol_only(conn, parsed, &filters, &mut selection)?;
                        } else {
                            Self::search_indexed(conn, parsed, &filters, &mut selection)?;
                        }
                        selection.finish()
                    }
                }
            };

            let mut items = Vec::with_capacity(page_ids.len());
            for id in page_ids {
                if let Some(meta) = Self::get_item_with_conn(conn, id)? {
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

    fn browse_items(
        conn: &Connection,
        filters: &SearchFilters,
        sort: SearchSort,
        page_size: u32,
        offset: u64,
    ) -> anyhow::Result<(u64, Vec<WorkshopId>)> {
        let total: u64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}"),
            params![filters.tag, filters.status, filters.public_only],
            |r| r.get(0),
        )?;

        let order_by = sort.browse_order();
        let sql = format!(
            "SELECT w.workshop_id FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}
             ORDER BY {order_by} LIMIT ?4 OFFSET ?5"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(
            params![
                filters.tag,
                filters.status,
                filters.public_only,
                page_size,
                offset
            ],
            |row| parse_workshop_id_col(0, row.get(0)?),
        )?;
        Ok((total, rows.collect::<Result<Vec<_>, _>>()?))
    }

    fn search_exact_id(
        conn: &Connection,
        filters: &SearchFilters,
        id: u64,
        offset: u64,
    ) -> anyhow::Result<(u64, Vec<WorkshopId>)> {
        let Ok(id) = i64::try_from(id) else {
            return Ok((0, Vec::new()));
        };
        let matched = conn
            .prepare_cached(&format!(
                "SELECT 1 FROM workshop_items w WHERE w.workshop_id = ?4 AND {SEARCH_ITEM_FILTERS}"
            ))?
            .exists(params![
                filters.tag,
                filters.status,
                filters.public_only,
                id
            ])?;
        let ids = if matched && offset == 0 {
            vec![WorkshopId(id as u64)]
        } else {
            Vec::new()
        };
        Ok((u64::from(matched), ids))
    }

    fn search_indexed(
        conn: &Connection,
        parsed: &ParsedQuery,
        filters: &SearchFilters,
        page: &mut SearchPage,
    ) -> anyhow::Result<()> {
        if let Some(fts_query) = search::build_primary_fts_query(parsed) {
            let (source, match_filter) = if parsed.numeric_prefix.is_some() {
                (
                    "FROM (
                         SELECT rowid AS workshop_id FROM workshop_items_fts WHERE workshop_items_fts MATCH ?4
                         UNION
                         SELECT workshop_id FROM workshop_items
                         WHERE workshop_id = ?5 OR CAST(workshop_id AS TEXT) LIKE ?6
                     ) candidates JOIN workshop_items w ON w.workshop_id = candidates.workshop_id",
                    "",
                )
            } else {
                (SEARCH_FTS_SOURCE, "AND workshop_items_fts MATCH ?4")
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
                    fts_query,
                    exact,
                    format!("{prefix}%")
                ])?
            } else {
                stmt.query(params![
                    filters.tag,
                    filters.status,
                    filters.public_only,
                    fts_query
                ])?
            };
            page.collect(rows, |row, id| {
                let title = row.get_ref(1)?.as_str()?;
                let creator = row.get_ref(5)?.as_str_or_null()?;
                Ok(search::score_primary_candidate(
                    parsed,
                    id.as_u64(),
                    title,
                    creator,
                ))
            })?;
        }

        if page.total == 0 {
            if let Some(desc_query) = search::build_desc_fts_query(parsed) {
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {SEARCH_ORDER_COLUMNS}, w.description {SEARCH_FTS_SOURCE}
                     WHERE workshop_items_fts MATCH ?4 AND {SEARCH_ITEM_FILTERS}"
                ))?;
                let rows = stmt.query(params![
                    filters.tag,
                    filters.status,
                    filters.public_only,
                    desc_query
                ])?;
                page.collect(rows, |row, _| {
                    let description = row.get_ref(5)?.as_str_or_null()?;
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
        page: &mut SearchPage,
    ) -> anyhow::Result<()> {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}"
        ))?;
        let rows = stmt.query(params![filters.tag, filters.status, filters.public_only])?;
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

        if page.total == 0 {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS}, w.description FROM workshop_items w
                 WHERE {SEARCH_ITEM_FILTERS} AND w.description IS NOT NULL"
            ))?;
            let rows = stmt.query(params![filters.tag, filters.status, filters.public_only])?;
            page.collect(rows, |row, _| {
                let description = row.get_ref(5)?.as_str()?;
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
