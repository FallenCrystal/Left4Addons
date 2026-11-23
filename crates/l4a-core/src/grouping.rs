use crate::db::LocalDb;
use crate::error::CoreError;
use crate::ops::{strip_suffix_ascii_case, toggle_addons};
use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AddonOperationOutcome, AddonSummary, BundleId, BundleKind, BundleRecord,
    GroupId, InstallState, LoadoutProfile, MapEntry, ProfileApplyReport, ProfileId,
    TrackedCollectionRecord, UserGroup, WorkshopId, WorkshopItemRecord,
};
use l4a_workshop::WorkshopResolver;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::OnceLock;

/// 按四层清晰模型管理：
/// 1. `Bundle`: 战役/地图多包物理整合（支持 `locked` 保护与跨分包地图/封面聚合）
/// 2. `TrackedCollection`: 上游工坊合集追踪
/// 3. `UserGroup`: 多对多自定义分类/标签
/// 4. `LoadoutProfile`: 一键启用预设
pub fn rebuild_auto_bundles(db: &LocalDb) -> Result<Vec<BundleRecord>, CoreError> {
    let existing = db.list_bundles()?;
    let mut locked_member_ids: HashSet<AddonId> = HashSet::new();
    for b in &existing {
        if b.locked || b.kind == BundleKind::Manual {
            for aid in &b.member_addon_ids {
                locked_member_ids.insert(aid.clone());
            }
        }
    }

    db.delete_unlocked_auto_bundles()?;

    let addons = db.list_addons()?;
    let snapshots = db.list_workshop_snapshots()?;
    let addon_map: HashMap<AddonId, AddonSummary> =
        addons.iter().map(|a| (a.id.clone(), a.clone())).collect();

    let candidates_by_id: HashMap<AddonId, BundleCandidate> = addons
        .iter()
        .filter(|a| a.install_state == InstallState::Installed)
        .map(|a| {
            let snap = a.workshop_id.and_then(|w| snapshots.get(&w));
            (a.id.clone(), BundleCandidate::from_addon(a, snap))
        })
        .collect();

    let wid_to_id: HashMap<WorkshopId, AddonId> = addons
        .iter()
        .filter(|a| a.install_state == InstallState::Installed)
        .filter_map(|a| a.workshop_id.map(|w| (w, a.id.clone())))
        .collect();

    let mut assigned: HashSet<AddonId> = locked_member_ids;

    // 第一阶段：智能战役依赖图聚合（仅限同一战役分包：排除非战役模组、排除第三方缺失材质修复/补丁/汉化、校验作者与标题词根一致性）
    let edges = db.list_all_active_dependency_edges()?;
    let mut adj: BTreeMap<AddonId, BTreeSet<AddonId>> = BTreeMap::new();
    for (parent_wid, dep_wid) in edges {
        let (Some(u_id), Some(v_id)) = (wid_to_id.get(&parent_wid), wid_to_id.get(&dep_wid)) else {
            continue;
        };
        if u_id == v_id || assigned.contains(u_id) || assigned.contains(v_id) {
            continue;
        }
        let (Some(u), Some(v)) = (candidates_by_id.get(u_id), candidates_by_id.get(v_id)) else {
            continue;
        };
        if is_valid_campaign_dependency_edge(u, v) {
            adj.entry(u_id.clone()).or_default().insert(v_id.clone());
            adj.entry(v_id.clone()).or_default().insert(u_id.clone());
        }
    }

    let mut visited: HashSet<AddonId> = HashSet::new();
    for start_id in adj.keys() {
        if visited.contains(start_id) || assigned.contains(start_id) {
            continue;
        }
        let mut comp_ids = Vec::new();
        let mut queue = VecDeque::new();
        visited.insert(start_id.clone());
        queue.push_back(start_id.clone());
        while let Some(curr) = queue.pop_front() {
            comp_ids.push(curr.clone());
            if let Some(neighbors) = adj.get(&curr) {
                for next in neighbors {
                    if !assigned.contains(next) && visited.insert(next.clone()) {
                        queue.push_back(next.clone());
                    }
                }
            }
        }

        if comp_ids.len() < 2 {
            continue;
        }
        let mut members: Vec<&BundleCandidate> = comp_ids
            .iter()
            .filter_map(|id| candidates_by_id.get(id))
            .collect();
        if !is_valid_campaign_group(&members) {
            continue;
        }
        sort_bundle_candidates(&mut members);
        let bundle_name = derive_bundle_name(&members);
        let primary_wid = members
            .iter()
            .find_map(|c| c.addon.workshop_id)
            .unwrap_or(WorkshopId(0));
        let member_ids: Vec<AddonId> = members.iter().map(|c| c.addon.id.clone()).collect();
        for aid in &member_ids {
            assigned.insert(aid.clone());
        }
        let bundle = assemble_bundle(
            BundleId(format!("bundle_dep_{primary_wid}")),
            bundle_name,
            BundleKind::DependencyGraph,
            false,
            member_ids,
            &addon_map,
        );
        db.save_bundle(&bundle)?;
    }

    // 收集剩余未分配的潜在战役分包候选（排除纯人物模型/表情、UI 及无显式分包特征的非战役模组）
    let remaining_candidates = |assigned_set: &HashSet<AddonId>| -> Vec<&BundleCandidate> {
        let mut list: Vec<&BundleCandidate> = candidates_by_id
            .values()
            .filter(|c| !assigned_set.contains(&c.addon.id) && c.eligible_for_campaign_bundle())
            .collect();
        list.sort_by(|a, b| a.addon.id.cmp(&b.addon.id));
        list
    };

    // 第二阶段 A：基于元数据标题（AddonTitle / WorkshopTitle）分包后缀聚合 + 战役主包自动挂载（v1 port）
    {
        let pool = remaining_candidates(&assigned);
        let mut by_root: BTreeMap<String, Vec<&BundleCandidate>> = BTreeMap::new();
        for cand in &pool {
            if let Some(part_info) = cand.metadata_part.as_ref() {
                if !cand.has_standalone_fix_marker_outside(&part_info.root_key) {
                    by_root
                        .entry(part_info.root_key.clone())
                        .or_default()
                        .push(*cand);
                }
            }
        }

        for (root_key, mut members) in by_root {
            // 挂载同名战役主包（例如 `City Escape` 或 `广西-南宁 V1.15.1` 挂载至 `Part 1/2` 或 `Textures`）
            for cand in &pool {
                if members.iter().any(|m| m.addon.id == cand.addon.id) {
                    continue;
                }
                if cand.matches_metadata_root(&root_key)
                    && !cand.has_standalone_fix_marker_outside(&root_key)
                    && members.iter().all(|m| authors_compatible(m, cand))
                {
                    members.push(*cand);
                }
            }

            if members.len() < 2 || !is_valid_campaign_group(&members) {
                continue;
            }
            sort_bundle_candidates(&mut members);
            let display_name = derive_bundle_name(&members);
            let author_sig = group_author_signature(&members);
            let member_ids: Vec<AddonId> = members.iter().map(|c| c.addon.id.clone()).collect();
            for aid in &member_ids {
                assigned.insert(aid.clone());
            }
            save_campaign_parts_bundle(
                db,
                &format!("title|{root_key}|{author_sig}"),
                display_name,
                member_ids,
                &addon_map,
            )?;
        }
    }

    // 第二阶段 B：基于本地文件名分包后缀聚合（处理本地附件战役包 Part 2/3 缺失 addoninfo.txt 或全部同名 addoninfo.txt 的场景）
    {
        let pool = remaining_candidates(&assigned);
        let mut by_file_root: BTreeMap<String, Vec<&BundleCandidate>> = BTreeMap::new();
        for cand in &pool {
            if let Some(fpart) = cand.file_part.as_ref() {
                if !is_generic_file_stem(&fpart.root_key)
                    && !cand.has_file_fix_marker_outside(&fpart.root_key)
                {
                    by_file_root
                        .entry(fpart.root_key.clone())
                        .or_default()
                        .push(*cand);
                }
            }
        }

        for (file_root_key, mut members) in by_file_root {
            for cand in &pool {
                if members.iter().any(|m| m.addon.id == cand.addon.id) {
                    continue;
                }
                if cand.file_clean_root_key.as_deref() == Some(file_root_key.as_str())
                    && !cand.has_file_fix_marker_outside(&file_root_key)
                    && members.iter().all(|m| authors_compatible(m, cand))
                {
                    members.push(*cand);
                }
            }

            if members.len() < 2
                || !is_valid_campaign_group(&members)
                || !metadata_titles_compatible(&members)
                || group_has_asymmetric_fix_marker(&members)
            {
                continue;
            }
            // 若仅靠裸数字尾缀（如 `foo_1.vpk`、`foo_2.vpk`）匹配且并非地图+资源互补分包，要求不存在版本号冲突
            if members
                .iter()
                .all(|m| m.file_part.as_ref().is_some_and(|p| p.bare_numeric))
                && has_conflicting_versions(&members)
                && !has_complementary_map_split(&members)
            {
                continue;
            }

            sort_bundle_candidates(&mut members);
            let display_name = derive_bundle_name(&members);
            let author_sig = group_author_signature(&members);
            let member_ids: Vec<AddonId> = members.iter().map(|c| c.addon.id.clone()).collect();
            for aid in &member_ids {
                assigned.insert(aid.clone());
            }
            save_campaign_parts_bundle(
                db,
                &format!("file|{file_root_key}|{author_sig}"),
                display_name,
                member_ids,
                &addon_map,
            )?;
        }
    }

    // 第二阶段 C：本地附件/工坊同名战役元数据分包聚合（v1 port + 本地附件近似模板兼容 + SDK 样板排除）
    {
        let pool = remaining_candidates(&assigned);
        let mut by_meta_title: BTreeMap<String, Vec<&BundleCandidate>> = BTreeMap::new();
        for cand in &pool {
            let Some(ref title_key) = cand.identical_group_title_key else {
                continue;
            };
            if cand.is_boilerplate_title
                || cand.is_standalone_fix_only_title
                || cand.has_file_fix_marker_outside(title_key)
            {
                continue;
            }
            by_meta_title
                .entry(title_key.clone())
                .or_default()
                .push(*cand);
        }

        // 先尝试处理 `<Root> Texture(s)` 同名材质分包挂载到 `<Root>` 主包的场景（v1 Northshore 用例）
        let keys: Vec<String> = by_meta_title.keys().cloned().collect();
        let mut consumed_keys: HashSet<String> = HashSet::new();
        for key in &keys {
            if consumed_keys.contains(key) {
                continue;
            }
            let Some(tex_root_key) = strip_texture_group_root_key(key) else {
                continue;
            };
            let Some(tex_members) = by_meta_title.get(key).cloned() else {
                continue;
            };
            if tex_members.len() < 2 || !group_authors_consistent(&tex_members) {
                continue;
            }
            let mut combined = tex_members;
            for cand in &pool {
                if combined.iter().any(|m| m.addon.id == cand.addon.id) {
                    continue;
                }
                if cand.identical_group_title_key.as_deref() == Some(tex_root_key.as_str())
                    && !cand.is_standalone_fix_only_title
                    && !cand.has_file_fix_marker_outside(&tex_root_key)
                    && combined.iter().all(|m| authors_compatible(m, cand))
                {
                    combined.push(*cand);
                }
            }
            if combined.len() >= 3
                && is_valid_campaign_group(&combined)
                && identical_title_group_corroborated(&combined)
            {
                consumed_keys.insert(key.clone());
                sort_bundle_candidates(&mut combined);
                let display_name = derive_bundle_name(&combined);
                let author_sig = group_author_signature(&combined);
                let member_ids: Vec<AddonId> =
                    combined.iter().map(|c| c.addon.id.clone()).collect();
                for aid in &member_ids {
                    assigned.insert(aid.clone());
                }
                save_campaign_parts_bundle(
                    db,
                    &format!("ident_tex|{tex_root_key}|{author_sig}"),
                    display_name,
                    member_ids,
                    &addon_map,
                )?;
            }
        }

        for (title_key, candidates_with_title) in by_meta_title {
            if consumed_keys.contains(&title_key) {
                continue;
            }
            let active: Vec<&BundleCandidate> = candidates_with_title
                .into_iter()
                .filter(|c| !assigned.contains(&c.addon.id))
                .collect();
            if active.len() < 2 {
                continue;
            }

            // 按兼容作者切分子簇（防止不同作者碰巧同名战役把整组拒绝或混入同一包）
            for mut cluster in partition_by_compatible_author(active) {
                if cluster.len() < 2
                    || (has_conflicting_versions(&cluster)
                        && !has_complementary_map_split(&cluster))
                    || !is_valid_campaign_group(&cluster)
                    || !identical_title_group_corroborated(&cluster)
                {
                    continue;
                }
                sort_bundle_candidates(&mut cluster);
                let display_name = derive_bundle_name(&cluster);
                let author_sig = group_author_signature(&cluster);
                let member_ids: Vec<AddonId> = cluster.iter().map(|c| c.addon.id.clone()).collect();
                for aid in &member_ids {
                    assigned.insert(aid.clone());
                }
                save_campaign_parts_bundle(
                    db,
                    &format!("ident|{title_key}|{author_sig}"),
                    display_name,
                    member_ids,
                    &addon_map,
                )?;
            }
        }
    }

    // 对所有已有手动/锁定 Bundle 刷新其聚合地图与封面
    for mut b in db.list_bundles()? {
        if b.locked || b.kind == BundleKind::Manual {
            b = assemble_bundle(
                b.id,
                b.name,
                b.kind,
                b.locked,
                b.member_addon_ids,
                &addon_map,
            );
            db.save_bundle(&b)?;
        }
    }

    db.list_bundles()
}

fn save_campaign_parts_bundle(
    db: &LocalDb,
    seed: &str,
    display_name: String,
    member_ids: Vec<AddonId>,
    addon_map: &HashMap<AddonId, AddonSummary>,
) -> Result<(), CoreError> {
    let digest = hex::encode(Sha256::digest(seed.as_bytes()));
    let bundle = assemble_bundle(
        BundleId(format!("bundle_camp_{}", &digest[..12])),
        display_name,
        BundleKind::CampaignParts,
        false,
        member_ids,
        addon_map,
    );
    db.save_bundle(&bundle)
}

#[derive(Debug, Clone)]
struct PartParse {
    root_display: String,
    root_key: String,
    order_rank: u32,
    bare_numeric: bool,
}

#[derive(Debug, Clone)]
struct BundleCandidate {
    addon: AddonSummary,
    has_campaign_signal: bool,
    exclusively_non_campaign: bool,
    is_pure_survivor_or_ui: bool,
    has_explicit_part_role: bool,
    steam_creator_id: Option<String>,
    author_name_key: Option<String>,
    description_key: Option<String>,
    tagline_key: Option<String>,
    url_key: Option<String>,
    version_key: Option<String>,
    clean_addon_title: Option<String>,
    clean_workshop_title: Option<String>,
    metadata_part: Option<PartParse>,
    metadata_root_keys: Vec<String>,
    metadata_display_roots: Vec<String>,
    file_stem: Option<String>,
    file_clean_root_key: Option<String>,
    file_part: Option<PartParse>,
    identical_group_title_key: Option<String>,
    is_boilerplate_title: bool,
    is_standalone_fix_only_title: bool,
    has_any_fix_marker: bool,
    has_metadata_fix_marker: bool,
    all_titles_lower: Vec<String>,
}

fn root_key_len_valid(key: &str) -> bool {
    key.chars().count() >= 3 || key.chars().filter(|c| !c.is_ascii()).count() >= 2
}

impl BundleCandidate {
    fn from_addon(addon: &AddonSummary, snap: Option<&WorkshopItemRecord>) -> Self {
        let snap_tags_campaign = snap.is_some_and(|s| {
            s.tags.iter().any(|t| {
                let lower = t.to_ascii_lowercase();
                lower.contains("campaign") || lower.contains("map")
            })
        });
        let has_campaign_signal = addon
            .categories
            .iter()
            .any(|c| c == "Campaign" || c == "Map")
            || !addon.maps.is_empty()
            || snap_tags_campaign;

        let has_non_campaign_major = addon
            .categories
            .iter()
            .any(|c| matches!(c.as_str(), "Survivor" | "Weapon" | "Infected" | "UI"));
        let exclusively_non_campaign = !has_campaign_signal && has_non_campaign_major;
        let is_pure_survivor_or_ui = !has_campaign_signal
            && addon
                .categories
                .iter()
                .any(|c| matches!(c.as_str(), "Survivor" | "UI"));

        let steam_creator_id = snap
            .and_then(|s| s.creator_steam_id.as_deref())
            .or_else(|| {
                addon
                    .addon_info
                    .get("addonauthorsteamid")
                    .map(String::as_str)
            })
            .map(str::trim)
            .filter(|s| {
                !s.is_empty()
                    && *s != "0"
                    && !s.eq_ignore_ascii_case("author_steam_id")
                    && !s.eq_ignore_ascii_case("steamid")
            })
            .map(str::to_ascii_lowercase);

        let author_name_key = addon
            .author
            .as_deref()
            .or_else(|| snap.and_then(|s| s.creator_name.as_deref()))
            .or_else(|| addon.addon_info.get("addonauthor").map(String::as_str))
            .and_then(normalize_non_placeholder_author);

        let description_key = addon
            .addon_info
            .get("addondescription")
            .or_else(|| addon.addon_info.get("description"))
            .map(String::as_str)
            .or_else(|| snap.and_then(|s| s.description.as_deref()))
            .map(normalize_comparison_text)
            .filter(|d| d.chars().count() > 10 && !is_boilerplate_description(d));

        let tagline_key = addon
            .addon_info
            .get("addontagline")
            .map(|s| normalize_comparison_text(s))
            .filter(|s| s.chars().count() >= 4 && !is_boilerplate_description(s));

        let url_key = addon
            .addon_info
            .get("addonurl0")
            .or_else(|| addon.addon_info.get("addonurl"))
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| s.len() > 8 && !s.contains("example.com") && !s.contains("valvesoftware"));

        let version_key = addon
            .version
            .as_deref()
            .or_else(|| addon.addon_info.get("addonversion").map(String::as_str))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_ascii_lowercase);

        let clean_addon_title = addon.addon_title.clone().or_else(|| {
            crate::db::repo::clean_addon_title(
                addon.addon_info.get("addontitle").map(String::as_str),
            )
        });
        let clean_workshop_title = addon.workshop_title.clone().or_else(|| {
            snap.and_then(|s| crate::db::repo::clean_workshop_title(Some(s.title.as_str())))
        });

        // 优先从 addoninfo.addontitle 提取分包信息（对齐 v1 优先级），其次从 Workshop 标题提取
        let metadata_part = clean_addon_title
            .as_deref()
            .and_then(|t| parse_part_from_title(t, false))
            .or_else(|| {
                clean_workshop_title
                    .as_deref()
                    .and_then(|t| parse_part_from_title(t, false))
            });

        let mut metadata_root_keys = Vec::new();
        let mut metadata_display_roots = Vec::new();
        for raw_title in [
            clean_addon_title.as_deref(),
            clean_workshop_title.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(p) = parse_part_from_title(raw_title, false) {
                push_unique_str(&mut metadata_root_keys, &p.root_key);
                push_unique_str(&mut metadata_display_roots, &p.root_display);
            }
            let base_clean = clean_root_title(&strip_trailing_version(raw_title));
            let base_key = normalize_comparison_text(&base_clean);
            if root_key_len_valid(&base_key) {
                push_unique_str(&mut metadata_root_keys, &base_key);
                push_unique_str(&mut metadata_display_roots, &base_clean);
            }
        }

        let file_stem = addon
            .current_file_name
            .as_deref()
            .map(strip_filename_decorations)
            .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()));

        let file_part = file_stem
            .as_deref()
            .and_then(|stem| parse_part_from_title(stem, true));

        let file_clean_root_key = file_stem
            .as_deref()
            .map(|s| normalize_comparison_text(&clean_root_title(&strip_trailing_version(s))))
            .filter(|k| root_key_len_valid(k));

        let has_explicit_part_role =
            metadata_part.is_some() || file_part.as_ref().is_some_and(|p| !p.bare_numeric);

        let primary_meta_for_identical = clean_addon_title
            .as_deref()
            .or(clean_workshop_title.as_deref());
        let identical_group_title_key = primary_meta_for_identical
            .map(|t| normalize_comparison_text(&clean_root_title(&strip_trailing_version(t))))
            .filter(|k| root_key_len_valid(k));

        let is_boilerplate_title = primary_meta_for_identical.is_some_and(is_sdk_boilerplate_title);
        let is_standalone_fix_only_title =
            primary_meta_for_identical.is_some_and(is_standalone_fix_title);

        let has_metadata_fix_marker = [
            clean_addon_title.as_deref(),
            clean_workshop_title.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(contains_fix_marker);

        let mut all_titles_lower = Vec::new();
        for s in [
            clean_addon_title.as_deref(),
            clean_workshop_title.as_deref(),
            Some(addon.title.as_str()),
            file_stem.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let lower = s.to_lowercase();
            if !all_titles_lower.contains(&lower) {
                all_titles_lower.push(lower);
            }
        }
        let has_any_fix_marker = all_titles_lower.iter().any(|t| contains_fix_marker(t));

        Self {
            addon: addon.clone(),
            has_campaign_signal,
            exclusively_non_campaign,
            is_pure_survivor_or_ui,
            has_explicit_part_role,
            steam_creator_id,
            author_name_key,
            description_key,
            tagline_key,
            url_key,
            version_key,
            clean_addon_title,
            clean_workshop_title,
            metadata_part,
            metadata_root_keys,
            metadata_display_roots,
            file_stem,
            file_clean_root_key,
            file_part,
            identical_group_title_key,
            is_boilerplate_title,
            is_standalone_fix_only_title,
            has_any_fix_marker,
            has_metadata_fix_marker,
            all_titles_lower,
        }
    }

    fn eligible_for_campaign_bundle(&self) -> bool {
        if !self.exclusively_non_campaign {
            return true;
        }
        // 允许战役的 Part 2/资源包同时携带自定义感染者或武器标记，但严禁纯人物模型/表情或 UI 模组混入
        self.has_explicit_part_role && !self.is_pure_survivor_or_ui
    }

    fn matches_metadata_root(&self, root_key: &str) -> bool {
        self.metadata_root_keys.iter().any(|k| k == root_key)
    }

    /// 检查除共享战役词根外的附加后缀中是否含有「修复 / 补丁 / 缺失材质 / 汉化」等独立补丁标记。
    fn has_standalone_fix_marker_outside(&self, shared_root_key: &str) -> bool {
        if !self.has_any_fix_marker {
            return false;
        }
        if contains_fix_marker(shared_root_key) {
            return false;
        }
        for raw in &self.all_titles_lower {
            let norm = normalize_comparison_text(raw);
            let remainder = norm.replace(shared_root_key, " ");
            if contains_fix_marker(&remainder) {
                return true;
            }
        }
        false
    }

    /// 仅针对本地文件名词干检查是否在文件词根之外额外附带了独立补丁标记（避免中英混合战役自身的「汉化版/Fixed」标题误伤）。
    fn has_file_fix_marker_outside(&self, file_root_key: &str) -> bool {
        let Some(ref stem) = self.file_stem else {
            return false;
        };
        if !contains_fix_marker(stem) || contains_fix_marker(file_root_key) {
            return false;
        }
        let norm = normalize_comparison_text(stem);
        let remainder = norm.replace(file_root_key, " ");
        contains_fix_marker(&remainder)
    }

    fn sort_rank(&self) -> (u32, u8, String) {
        let default_rank = if !self.addon.maps.is_empty() || self.has_campaign_signal {
            0
        } else {
            500
        };
        let part_rank = self
            .metadata_part
            .as_ref()
            .map(|p| p.order_rank)
            .or_else(|| self.file_part.as_ref().map(|p| p.order_rank))
            .unwrap_or(default_rank);
        let map_priority = if !self.addon.maps.is_empty() { 0 } else { 1 };
        let tie = self
            .clean_workshop_title
            .as_deref()
            .or(self.clean_addon_title.as_deref())
            .or(self.addon.current_file_name.as_deref())
            .unwrap_or(&self.addon.title)
            .to_lowercase();
        (part_rank, map_priority, tie)
    }
}

fn is_valid_campaign_dependency_edge(u: &BundleCandidate, v: &BundleCandidate) -> bool {
    if !u.eligible_for_campaign_bundle() || !v.eligible_for_campaign_bundle() {
        return false;
    }
    if !authors_compatible(u, v) {
        return false;
    }
    let Some(shared_root) = shared_campaign_root_key(u, v) else {
        return false;
    };
    !u.has_standalone_fix_marker_outside(&shared_root)
        && !v.has_standalone_fix_marker_outside(&shared_root)
}

fn shared_campaign_root_key(u: &BundleCandidate, v: &BundleCandidate) -> Option<String> {
    for uk in &u.metadata_root_keys {
        if root_key_len_valid(uk) && v.metadata_root_keys.iter().any(|vk| vk == uk) {
            return Some(uk.clone());
        }
    }
    // 支持如 `Chernobyl: Chapter One` 与 `Chernobyl Assets Pack` 这类共享战役前缀的分包关联
    for uk in &u.metadata_root_keys {
        for vk in &v.metadata_root_keys {
            if let Some(prefix) = shared_part_prefix_key(uk, vk) {
                return Some(prefix);
            }
        }
    }
    if u.metadata_root_keys.is_empty() || v.metadata_root_keys.is_empty() {
        let u_file = u
            .file_part
            .as_ref()
            .map(|p| p.root_key.as_str())
            .or(u.file_clean_root_key.as_deref());
        let v_file = v
            .file_part
            .as_ref()
            .map(|p| p.root_key.as_str())
            .or(v.file_clean_root_key.as_deref());
        if let (Some(uf), Some(vf)) = (u_file, v_file) {
            if uf == vf && root_key_len_valid(uf) && !is_generic_file_stem(uf) {
                return Some(uf.to_string());
            }
        }
    }
    None
}

fn shared_part_prefix_key(a: &str, b: &str) -> Option<String> {
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if !root_key_len_valid(shorter) {
        return None;
    }
    let rest = longer.strip_prefix(shorter)?.trim();
    if rest.is_empty() {
        return Some(shorter.to_string());
    }
    let trimmed_rest = rest.trim_start_matches([':', '-', '_', '/', '|', '(', '[', ' ']);
    is_recognized_subpackage_qualifier(trimmed_rest).then(|| shorter.to_string())
}

fn is_recognized_subpackage_qualifier(s: &str) -> bool {
    static RE_QUAL: OnceLock<Regex> = OnceLock::new();
    let re = RE_QUAL.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:(?:part|pt|partie|pts|pack|vol|volume|chapter|ch|ep|episode)\s*(?:\d+|[ivxldcm]+|one|two|three|four|five|six|seven|eight|nine|ten)?(?:\s*[/\-]\s*\d+)?|(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|base|media|props|content)(?:\s*(?:&|and|/|\+|、|与|及)\s*(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|base|media|props|content))*|分包\s*\d*|第\s*[\d一二三四五六七八九十]+\s*[部章分卷包]|(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包)(?:\s*(?:&|/|、|与|及)\s*(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包))*)[\s\)\]]*$",
        )
        .unwrap()
    });
    re.is_match(s.trim())
}

fn is_valid_campaign_group(members: &[&BundleCandidate]) -> bool {
    if members.len() < 2 {
        return false;
    }
    if members.iter().any(|m| !m.eligible_for_campaign_bundle()) {
        return false;
    }
    if !members.iter().any(|m| m.has_campaign_signal) {
        return false;
    }
    group_authors_consistent(members)
}

fn authors_compatible(a: &BundleCandidate, b: &BundleCandidate) -> bool {
    if let (Some(sa), Some(sb)) = (&a.steam_creator_id, &b.steam_creator_id) {
        return sa == sb;
    }
    if let (Some(na), Some(nb)) = (&a.author_name_key, &b.author_name_key) {
        return na == nb;
    }
    true
}

fn group_authors_consistent(members: &[&BundleCandidate]) -> bool {
    let steam_ids: HashSet<&str> = members
        .iter()
        .filter_map(|m| m.steam_creator_id.as_deref())
        .collect();
    if steam_ids.len() > 1 {
        return false;
    }
    if steam_ids.is_empty() {
        let names: HashSet<&str> = members
            .iter()
            .filter_map(|m| m.author_name_key.as_deref())
            .collect();
        if names.len() > 1 {
            return false;
        }
    }
    true
}

fn group_author_signature(members: &[&BundleCandidate]) -> String {
    if let Some(sid) = members.iter().find_map(|m| m.steam_creator_id.as_deref()) {
        return format!("sid:{sid}");
    }
    if let Some(name) = members.iter().find_map(|m| m.author_name_key.as_deref()) {
        return format!("author:{name}");
    }
    String::new()
}

fn partition_by_compatible_author<'a>(
    candidates: Vec<&'a BundleCandidate>,
) -> Vec<Vec<&'a BundleCandidate>> {
    let mut clusters: Vec<Vec<&'a BundleCandidate>> = Vec::new();
    for cand in candidates {
        if let Some(cluster) = clusters
            .iter_mut()
            .find(|cl| cl.iter().all(|existing| authors_compatible(existing, cand)))
        {
            cluster.push(cand);
        } else {
            clusters.push(vec![cand]);
        }
    }
    clusters
}

fn metadata_titles_compatible(members: &[&BundleCandidate]) -> bool {
    let titled: Vec<&BundleCandidate> = members
        .iter()
        .copied()
        .filter(|m| !m.metadata_root_keys.is_empty())
        .collect();
    if titled.len() <= 1 {
        return true;
    }
    let first = titled[0];
    titled[1..]
        .iter()
        .all(|other| shared_campaign_root_key(first, other).is_some())
}

/// 若同组有元数据标题的成员中，仅部分成员含有「修复/补丁/汉化」标记而其他成员没有，判定为独立补丁混入并拒绝。
fn group_has_asymmetric_fix_marker(members: &[&BundleCandidate]) -> bool {
    let titled: Vec<&BundleCandidate> = members
        .iter()
        .copied()
        .filter(|m| m.clean_addon_title.is_some() || m.clean_workshop_title.is_some())
        .collect();
    if titled.len() < 2 {
        return false;
    }
    let with_fix = titled.iter().filter(|m| m.has_metadata_fix_marker).count();
    with_fix > 0 && with_fix < titled.len()
}

fn has_conflicting_versions(members: &[&BundleCandidate]) -> bool {
    let versions: HashSet<&str> = members
        .iter()
        .filter_map(|m| m.version_key.as_deref())
        .collect();
    versions.len() > 1
}

fn has_complementary_map_split(members: &[&BundleCandidate]) -> bool {
    members.iter().any(|m| !m.addon.maps.is_empty())
        && (members.iter().any(|m| m.addon.maps.is_empty()) || !all_map_sets_identical(members))
}

/// 校验本地附件/工坊同名战役包是否确实属于同一套战役的分包（避免两个碰巧同名的独立地图误合并）。
fn identical_title_group_corroborated(members: &[&BundleCandidate]) -> bool {
    let has_shared_author = members
        .iter()
        .filter(|m| m.steam_creator_id.is_some() || m.author_name_key.is_some())
        .count()
        >= 2;

    let same_non_empty_field = |extract: fn(&BundleCandidate) -> Option<&str>| -> bool {
        let vals: Vec<&str> = members.iter().filter_map(|m| extract(m)).collect();
        vals.len() >= 2 && vals.windows(2).all(|w| w[0] == w[1])
    };

    let same_desc = same_non_empty_field(|m| m.description_key.as_deref());
    let same_tagline = same_non_empty_field(|m| m.tagline_key.as_deref());
    let same_url = same_non_empty_field(|m| m.url_key.as_deref());

    let shared_file_prefix = {
        let stems: Vec<String> = members
            .iter()
            .filter_map(|m| m.file_stem.as_deref())
            .map(normalize_comparison_text)
            .filter(|s| root_key_len_valid(s))
            .collect();
        if stems.len() >= 2 {
            let mut prefix_chars: Vec<char> = stems[0].chars().collect();
            for other in &stems[1..] {
                let common = prefix_chars
                    .iter()
                    .zip(other.chars())
                    .take_while(|(ca, cb)| **ca == *cb)
                    .count();
                prefix_chars.truncate(common);
            }
            let raw_prefix: String = prefix_chars.into_iter().collect();
            let clean_prefix = raw_prefix
                .trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, ' ' | '_' | '-'))
                .trim();
            !is_generic_file_stem(clean_prefix)
                && (clean_prefix.chars().count() >= 4
                    || clean_prefix.chars().filter(|c| !c.is_ascii()).count() >= 2)
        } else {
            false
        }
    };

    // 本地分包常见结构：至少一个包带地图 (.bsp/mission)，其余同作者同名包为纯材质/模型资源包 (maps 为空) 或互补章节地图
    let complementary_map_split = has_shared_author && has_complementary_map_split(members);

    if has_shared_author
        && (same_desc || same_tagline || same_url || shared_file_prefix || complementary_map_split)
    {
        return true;
    }

    // 即使本地附件作者未填写 author，若非样板标题完全一致且简介/标语/链接完全相同或文件名前缀一致，也归为同一套分包
    same_desc || same_tagline || same_url || shared_file_prefix
}

fn all_map_sets_identical(members: &[&BundleCandidate]) -> bool {
    let sets: Vec<BTreeSet<String>> = members
        .iter()
        .map(|m| {
            m.addon
                .maps
                .iter()
                .map(|entry| entry.code.to_ascii_lowercase())
                .collect()
        })
        .collect();
    sets.windows(2).all(|w| !w[0].is_empty() && w[0] == w[1])
}

fn sort_bundle_candidates(members: &mut [&BundleCandidate]) {
    members.sort_by(|a, b| {
        a.sort_rank()
            .cmp(&b.sort_rank())
            .then_with(|| a.addon.id.cmp(&b.addon.id))
    });
}

/// 统一生成纯净的战役整合包标题，绝不混入裸文件名或 `[workshop_id]` / `Part 1` 尾巴。
fn derive_bundle_name(members: &[&BundleCandidate]) -> String {
    // 1. 优先从元数据分包解析出的干净词根中选取（且优先带战役/地图信号的成员）
    for prefer_campaign in [true, false] {
        for m in members {
            if prefer_campaign && !m.has_campaign_signal {
                continue;
            }
            if let Some(ref p) = m.metadata_part {
                if p.root_display.chars().count() >= 2 {
                    return p.root_display.clone();
                }
            }
        }
    }

    // 2. 若成员间存在共享的干净元数据词根（如 `Chernobyl: Chapter One` + `Chernobyl Assets Pack` -> `Chernobyl` 或主包干净标题）
    if members.len() >= 2 {
        let mut root_counts: HashMap<String, (usize, String)> = HashMap::new();
        for m in members {
            for disp in &m.metadata_display_roots {
                let key = normalize_comparison_text(disp);
                let entry = root_counts.entry(key).or_insert_with(|| (0, disp.clone()));
                entry.0 += 1;
                if disp.len() < entry.1.len()
                    && root_key_len_valid(&normalize_comparison_text(disp))
                {
                    entry.1 = disp.clone();
                }
            }
        }
        if let Some((_, best_disp)) = root_counts
            .into_values()
            .filter(|(cnt, _)| *cnt >= 2)
            .max_by_key(|(cnt, disp)| (*cnt, std::cmp::Reverse(disp.len())))
        {
            return best_disp;
        }
    }

    // 3. 退回到带地图/战役信号的主包元数据标题（去除尾部分包字样与多余符号，绝不用文件名）
    for prefer_campaign in [true, false] {
        for m in members {
            if prefer_campaign && !m.has_campaign_signal {
                continue;
            }
            if let Some(raw) = m
                .clean_addon_title
                .as_deref()
                .or(m.clean_workshop_title.as_deref())
            {
                let cleaned = clean_root_title(&strip_trailing_version(raw));
                if !cleaned.is_empty() {
                    return cleaned;
                }
                return raw.trim().to_string();
            }
        }
    }

    // 4. 仅当整包内所有成员均无任何元数据标题时，才使用清洗后的文件名公共词根
    for m in members {
        if let Some(ref fp) = m.file_part {
            if !fp.root_display.is_empty() {
                return fp.root_display.clone();
            }
        }
        if let Some(ref stem) = m.file_stem {
            let cleaned = clean_root_title(stem);
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
    }

    members[0].addon.title.clone()
}

fn parse_part_from_title(raw: &str, allow_bare_numeric: bool) -> Option<PartParse> {
    let trimmed = raw.trim();
    if trimmed.chars().count() < 3 {
        return None;
    }

    if let Some(parsed) = parse_explicit_part_from_str(trimmed) {
        return Some(parsed);
    }

    let without_trailing_ver = strip_trailing_version(trimmed);
    if without_trailing_ver != trimmed && without_trailing_ver.chars().count() >= 3 {
        if let Some(parsed) = parse_explicit_part_from_str(without_trailing_ver.trim()) {
            return Some(parsed);
        }
    }

    // 5. 仅限本地文件名的裸序号尾缀：`yinghua_1` / `yinghua_2`、`樱花_1` / `樱花_2` 或 `campaign (1)`
    if allow_bare_numeric {
        static RE_BARE_NUM: OnceLock<Regex> = OnceLock::new();
        let re_num = RE_BARE_NUM
            .get_or_init(|| Regex::new(r"^(.*?)(?:[\-_]+|\s+\()0*([1-9]\d?)\)?$").unwrap());
        if let Some(caps) = re_num.captures(trimmed) {
            let raw_root = caps.get(1).map_or("", |m| m.as_str());
            // 排除如 `campaign_v1` / `mod_v2` 的版本号前缀
            if !raw_root.to_ascii_lowercase().ends_with('v') {
                let rank = caps
                    .get(2)
                    .and_then(|m| m.as_str().parse::<u32>().ok())
                    .unwrap_or(1);
                if let Some(parsed) = finalize_part_parse(raw_root, rank, true) {
                    return Some(parsed);
                }
            }
        }
    }

    None
}

fn parse_explicit_part_from_str(trimmed: &str) -> Option<PartParse> {
    static RE_BRACKET_PART: OnceLock<Regex> = OnceLock::new();
    static RE_WORD_PART: OnceLock<Regex> = OnceLock::new();
    static RE_CN_PART: OnceLock<Regex> = OnceLock::new();
    static RE_ASSET_PART: OnceLock<Regex> = OnceLock::new();

    // 1. 括号/方括号包裹的分包后缀：`Yama (Part 1 - Maps)`、`Yama (Models & Materials)`、`Campaign [1/3]`
    let re_bracket = RE_BRACKET_PART.get_or_init(|| {
        Regex::new(
            r"(?i)^(.*?)[\s\-_#:]*[\(\[\{]\s*(?:(?:part|pt|partie|pts|pack|vol|volume|chapter|ch|ep|episode)\s*(\d+|[ivx]+|one|two|three|four|five|six|seven|eight|nine|ten)(?:\s*[/\-]\s*\d+)?(?:\s*[-:–]\s*[^\)\]\}]*)?|(\d+)\s*/\s*\d+|(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|props|media)(?:\s*(?:&|and|/|\+|、|与|及)\s*(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|props|media))*|分包\s*(\d*)|(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包)(?:\s*(?:&|/|、|与|及)\s*(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包))*)\s*[\)\]\}]\s*$",
        )
        .unwrap()
    });
    if let Some(caps) = re_bracket.captures(trimmed) {
        let raw_root = caps.get(1).map_or("", |m| m.as_str());
        let idx_str = caps
            .get(2)
            .or_else(|| caps.get(3))
            .or_else(|| caps.get(4))
            .map(|m| m.as_str());
        let full_suffix = caps.get(0).map_or("", |m| &m.as_str()[raw_root.len()..]);
        let lower_suffix = full_suffix.to_lowercase();
        let fallback_rank = if lower_suffix.contains("map")
            || lower_suffix.contains("main")
            || lower_suffix.contains("主包")
            || lower_suffix.contains("地图")
        {
            0
        } else {
            100
        };
        let rank = idx_str.and_then(parse_part_number).unwrap_or(fallback_rank);
        if let Some(parsed) = finalize_part_parse(raw_root, rank, false) {
            return Some(parsed);
        }
    }

    // 2. 显式英文分包词：`Part 1`、`Pt. II`、`Chapter One`、`Part 2 - Models & Materials`
    let re_word = RE_WORD_PART.get_or_init(|| {
        Regex::new(
            r"(?i)^(.*?)(?:[\s\-_#/:,]+|\s+)(?:part|pt|partie|pts|pack|vol|volume|chapter|ch|ep|episode)\.?\s*(\d+|[ivx]+|one|two|three|four|five|six|seven|eight|nine|ten)(?:\s*/\s*\d+)?(?:\s*[-:–(]\s*(?:(?:maps?|models?|materials?|textures?|sounds?|audio|music|assets?|resources?|main|props|media)(?:\s*(?:&|and|/|\+)\s*(?:maps?|models?|materials?|textures?|sounds?|audio|music|assets?|resources?|main|props|media))*)[\)\s]*)?$",
        )
        .unwrap()
    });
    if let Some(caps) = re_word.captures(trimmed) {
        let raw_root = caps.get(1).map_or("", |m| m.as_str());
        let rank = caps
            .get(2)
            .and_then(|m| parse_part_number(m.as_str()))
            .unwrap_or(1);
        if let Some(parsed) = finalize_part_parse(raw_root, rank, false) {
            return Some(parsed);
        }
    }

    // 3. 中文分包词：`分包1`、`第1部分`、`材质包`、`模型包`、`音效包`、`主包`
    let re_cn = RE_CN_PART.get_or_init(|| {
        Regex::new(
            r"(?i)^(.*?)[\s\-_#:：]*(?:分[包卷]\s*(\d+|[一二三四五六七八九十]+)|第\s*(\d+|[一二三四五六七八九十]+)\s*[部章分卷包]|((?:主包|地图包|材质包|贴图包|模型包|音效包|音频包|资源包)(?:\s*(?:&|/|、|与|及)\s*(?:主包|地图包|材质包|贴图包|模型包|音效包|音频包|资源包))*))$",
        )
        .unwrap()
    });
    if let Some(caps) = re_cn.captures(trimmed) {
        let raw_root = caps.get(1).map_or("", |m| m.as_str());
        let rank = caps
            .get(2)
            .or_else(|| caps.get(3))
            .and_then(|m| parse_part_number(m.as_str()))
            .unwrap_or_else(|| match caps.get(4).map(|m| m.as_str()) {
                Some("主包" | "地图包") => 0,
                _ => 120,
            });
        if let Some(parsed) = finalize_part_parse(raw_root, rank, false) {
            return Some(parsed);
        }
    }

    // 4. 英文资源角色后缀：`Northshore Textures`、`Chernobyl Assets Pack`、`Yama - Materials and Models`
    let re_asset = RE_ASSET_PART.get_or_init(|| {
        Regex::new(
            r"(?i)^(.*?)(?:[\s\-_:]+)((?:textures?|materials?|models?|sounds?|audio|assets(?:\s+pack)?|resources(?:\s+pack)?)(?:\s*(?:&|and|/|\+)\s*(?:textures?|materials?|models?|sounds?|audio|assets(?:\s+pack)?|resources(?:\s+pack)?))*)(?:[\s\-_]*(\d+))?$",
        )
        .unwrap()
    });
    if let Some(caps) = re_asset.captures(trimmed) {
        let raw_root = caps.get(1).map_or("", |m| m.as_str());
        let base_rank = match caps
            .get(2)
            .map(|m| m.as_str().to_ascii_lowercase())
            .as_deref()
        {
            Some(s) if s.starts_with("model") => 110,
            Some(s) if s.starts_with("material") || s.starts_with("texture") => 120,
            Some(s) if s.starts_with("sound") || s.starts_with("audio") => 130,
            _ => 140,
        };
        let offset = caps
            .get(3)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if let Some(parsed) = finalize_part_parse(raw_root, base_rank + offset, false) {
            return Some(parsed);
        }
    }

    None
}

fn finalize_part_parse(raw_root: &str, order_rank: u32, bare_numeric: bool) -> Option<PartParse> {
    let without_ver = strip_trailing_version(&clean_root_title(raw_root));
    let root_display = clean_root_title(&without_ver);
    let root_key = normalize_comparison_text(&root_display);
    if !root_key_len_valid(&root_key) {
        return None;
    }
    Some(PartParse {
        root_display,
        root_key,
        order_rank,
        bare_numeric,
    })
}

fn parse_part_number(raw: &str) -> Option<u32> {
    let s = raw.trim().to_ascii_lowercase();
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    match s.as_str() {
        "i" | "one" | "一" => Some(1),
        "ii" | "two" | "二" => Some(2),
        "iii" | "three" | "三" => Some(3),
        "iv" | "four" | "四" => Some(4),
        "v" | "five" | "五" => Some(5),
        "vi" | "six" | "六" => Some(6),
        "vii" | "seven" | "七" => Some(7),
        "viii" | "eight" | "八" => Some(8),
        "ix" | "nine" | "九" => Some(9),
        "x" | "ten" | "十" => Some(10),
        _ => None,
    }
}

fn strip_trailing_version(raw: &str) -> String {
    static RE_VER: OnceLock<Regex> = OnceLock::new();
    let re = RE_VER.get_or_init(|| {
        Regex::new(r"(?i)[\s\-_]*(?:\(\s*)?(?:v|ver|version)\.?\s*\d+(?:\.\d+)*[a-z]?(?:\s*\))?$")
            .unwrap()
    });
    let stripped = re.replace(raw.trim(), "");
    stripped.into_owned()
}

fn strip_filename_decorations(file_name: &str) -> String {
    let mut s = strip_suffix_ascii_case(file_name.trim(), ".disabled").unwrap_or(file_name.trim());
    s = strip_suffix_ascii_case(s, ".vpk").unwrap_or(s);
    let mut rest = s.trim();
    // 剥离重命名规则添加的前缀如 `[2814104912] [Campaigns] Title`
    while let Some(after_open) = rest.strip_prefix('[') {
        if let Some((_, tail)) = after_open.split_once(']') {
            let trimmed_tail = tail.trim();
            if !trimmed_tail.is_empty() {
                rest = trimmed_tail;
                continue;
            }
        }
        break;
    }
    rest.trim().to_string()
}

fn strip_texture_group_root_key(title_key: &str) -> Option<String> {
    let stripped = title_key
        .strip_suffix(" textures")
        .or_else(|| title_key.strip_suffix(" texture"))
        .or_else(|| title_key.strip_suffix(" materials"))
        .map(str::trim)?;
    root_key_len_valid(stripped).then(|| stripped.to_string())
}

fn normalize_comparison_text(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if matches!(c, '_' | '-' | ':' | '/' | '\\' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_non_placeholder_author(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    if matches!(
        lower.as_str(),
        "unknown"
            | "unknown author"
            | "author"
            | "author_name"
            | "addonauthor"
            | "your name"
            | "valve"
            | "local"
            | "none"
            | "n/a"
    ) {
        return None;
    }
    Some(lower)
}

fn is_sdk_boilerplate_title(raw: &str) -> bool {
    let norm = normalize_comparison_text(raw);
    matches!(
        norm.as_str(),
        "dead line"
            | "dead line 2"
            | "deadline"
            | "my addon"
            | "my add on"
            | "my first addon"
            | "my first add on"
            | "addon"
            | "addon title"
            | "addontitle"
            | "addon_title"
            | "untitled"
            | "sample"
            | "sample campaign"
            | "campaign"
            | "custom campaign"
            | "new campaign"
            | "map"
            | "custom map"
            | "template"
            | "example"
            | "test"
            | "left 4 dead 2"
            | "l4d2"
    )
}

fn is_boilerplate_description(norm_desc: &str) -> bool {
    norm_desc.contains("dead line add on")
        || norm_desc.contains("short description of your add on")
        || norm_desc.contains("enter a description")
}

fn is_generic_file_stem(root_key: &str) -> bool {
    matches!(
        root_key,
        "" | "map"
            | "maps"
            | "campaign"
            | "addon"
            | "addons"
            | "mod"
            | "mods"
            | "vpk"
            | "part"
            | "pack"
            | "test"
            | "local"
            | "custom"
            | "left4dead2"
            | "left 4 dead 2"
            | "l4d2"
    )
}

fn contains_fix_marker(text: &str) -> bool {
    static RE_FIX_EN: OnceLock<Regex> = OnceLock::new();
    let lower = text.to_lowercase();
    for kw in [
        "修复",
        "补丁",
        "缺失材质",
        "材质修复",
        "贴图修复",
        "汉化",
        "字幕",
        "本地化",
        "魔改",
    ] {
        if lower.contains(kw) {
            return true;
        }
    }
    let re = RE_FIX_EN.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:fix|fixed|fixes|hotfix|bugfix|patch|patched|missing\s+textures?|missing\s+materials?|nav\s*fix|skybox\s+fix|translation|localization|subtitles?)\b",
        )
        .unwrap()
    });
    re.is_match(&lower)
}

/// 判断标题是否本身就是一个独立的通用修复补丁名称（如 `地图缺失材质修复`、`Missing Texture Fix`），而非带有 `(汉化修复版)` / `(Fixed)` 的真实战役名。
fn is_standalone_fix_title(raw: &str) -> bool {
    if !contains_fix_marker(raw) {
        return false;
    }
    static RE_STRIP_FIX: OnceLock<Regex> = OnceLock::new();
    let re = RE_STRIP_FIX.get_or_init(|| {
        Regex::new(
            r"(?i)(?:缺失材质|材质修复|贴图修复|修复|补丁|汉化|字幕|本地化|魔改|\b(?:missing\s+textures?|missing\s+materials?|nav\s*fix|skybox\s+fix|hotfix|bugfix|fixes|fixed|fix|patched|patch|translation|localization|subtitles?|maps?|campaign|addon|mod|for|of|the|版)\b|地图|战役|模组)",
        )
        .unwrap()
    });
    let normalized = normalize_comparison_text(raw);
    let stripped = re.replace_all(&normalized, " ");
    let meaningful_chars = stripped.chars().filter(|c| c.is_alphanumeric()).count();
    meaningful_chars < 2
}

fn push_unique_str(vec: &mut Vec<String>, val: &str) {
    if !val.is_empty() && !vec.iter().any(|existing| existing == val) {
        vec.push(val.to_string());
    }
}

pub fn create_manual_bundle(
    db: &LocalDb,
    name: String,
    member_addon_ids: Vec<AddonId>,
) -> Result<BundleRecord, CoreError> {
    let addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();
    let bundle = assemble_bundle(
        BundleId::generate(),
        name,
        BundleKind::Manual,
        true,
        member_addon_ids,
        &addons,
    );
    db.save_bundle(&bundle)?;
    Ok(bundle)
}

pub fn set_bundle_locked(db: &LocalDb, id: &BundleId, locked: bool) -> Result<(), CoreError> {
    db.set_bundle_locked(id, locked)
}

pub fn delete_bundle(db: &LocalDb, id: &BundleId) -> Result<(), CoreError> {
    db.delete_bundle(id)
}

fn assemble_bundle(
    id: BundleId,
    name: String,
    kind: BundleKind,
    locked: bool,
    member_addon_ids: Vec<AddonId>,
    addon_map: &HashMap<AddonId, AddonSummary>,
) -> BundleRecord {
    let mut aggregated_maps: Vec<MapEntry> = Vec::new();
    let mut seen_codes: HashSet<String> = HashSet::new();
    let mut cover_image: Option<String> = None;

    // 优先使用含地图主包的封面，其次回退至任意分包封面
    for prefer_with_maps in [true, false] {
        if cover_image.is_some() {
            break;
        }
        for aid in &member_addon_ids {
            let Some(addon) = addon_map.get(aid) else {
                continue;
            };
            if prefer_with_maps && addon.maps.is_empty() {
                continue;
            }
            if addon.cover_image.is_some() {
                cover_image = addon.cover_image.clone();
                break;
            }
        }
    }

    for aid in &member_addon_ids {
        let Some(addon) = addon_map.get(aid) else {
            continue;
        };
        for m in &addon.maps {
            let key = m.code.to_ascii_lowercase();
            if seen_codes.insert(key.clone()) {
                aggregated_maps.push(m.clone());
            } else if let Some(existing) = aggregated_maps
                .iter_mut()
                .find(|x| x.code.eq_ignore_ascii_case(&key))
            {
                if existing.image_path.is_none() && m.image_path.is_some() {
                    existing.image_path = m.image_path.clone();
                }
                if existing.name == existing.code && m.name != m.code {
                    existing.name = m.name.clone();
                }
            }
        }
    }

    BundleRecord {
        id,
        name,
        kind,
        locked,
        member_addon_ids,
        maps: aggregated_maps,
        cover_image,
    }
}

fn clean_root_title(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    ':' | '：' | '_' | '-' | '–' | '—' | '/' | '\\' | '#' | '+' | ',' | '.'
                )
        })
        .trim()
        .to_string()
}

pub async fn track_workshop_collection(
    db: &LocalDb,
    resolver: &WorkshopResolver,
    collection_id: WorkshopId,
    auto_sync: bool,
) -> Result<TrackedCollectionRecord, CoreError> {
    let (remote, _warnings) = resolver.fetch_collection(collection_id).await?;
    let _workspace = db.lock_workspace()?;
    let mut removed_upstream_ids = db
        .list_tracked_collections()?
        .into_iter()
        .find(|col| col.collection_id == collection_id)
        .map(|col| col.removed_upstream_ids)
        .unwrap_or_default();
    removed_upstream_ids.retain(|id| !remote.child_ids.contains(id));
    let record = TrackedCollectionRecord {
        collection_id,
        title: remote
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Collection #{collection_id}")),
        description: remote.description,
        auto_sync,
        member_workshop_ids: remote.child_ids,
        removed_upstream_ids,
        last_synced_at: Some(Utc::now()),
    };
    db.write(|tx| {
        for item in &remote.items {
            LocalDb::upsert_workshop_snapshot_tx(tx, item, false)?;
        }
        LocalDb::save_tracked_collection_tx(tx, &record)
    })?;
    Ok(record)
}

pub fn untrack_workshop_collection(
    db: &LocalDb,
    collection_id: WorkshopId,
) -> Result<(), CoreError> {
    let _workspace = db.lock_workspace()?;
    db.delete_tracked_collection(collection_id)
}

pub fn save_user_group(db: &LocalDb, group: UserGroup) -> Result<UserGroup, CoreError> {
    db.save_user_group(&group)?;
    Ok(group)
}

pub fn delete_user_group(db: &LocalDb, id: &GroupId) -> Result<(), CoreError> {
    db.delete_user_group(id)
}

pub fn save_profile(db: &LocalDb, profile: LoadoutProfile) -> Result<LoadoutProfile, CoreError> {
    db.save_profile(&profile)?;
    Ok(profile)
}

pub fn delete_profile(db: &LocalDb, id: &ProfileId) -> Result<(), CoreError> {
    db.delete_profile(id)
}

/// 一键应用预设方案 (仅切换 `loading/` 目录中的模组)。
/// 跳过 `workshop/` 目录中的模组，以免触发 Steam 重新下载。
pub fn apply_profile(
    db: &LocalDb,
    profile_id: &ProfileId,
    image_cache_dir: &Path,
    config: &l4a_protocol::AppConfig,
) -> Result<ProfileApplyReport, CoreError> {
    let profiles = db.list_profiles()?;
    let profile = profiles
        .into_iter()
        .find(|p| &p.id == profile_id)
        .ok_or_else(|| CoreError::NotFound(format!("预设不存在: {profile_id}")))?;

    let desired: HashSet<AddonId> = profile.enabled_addon_ids.iter().cloned().collect();
    let addons = db.list_addons()?;
    let by_id: HashMap<AddonId, AddonSummary> =
        addons.iter().map(|a| (a.id.clone(), a.clone())).collect();

    let mut missing_addons = Vec::new();
    let mut skipped_workshop_only = Vec::new();
    let mut to_enable = Vec::new();
    let mut to_disable = Vec::new();

    for aid in &desired {
        match by_id.get(aid) {
            None => missing_addons.push(aid.clone()),
            Some(a) if a.install_state != InstallState::Installed => {
                missing_addons.push(aid.clone())
            }
            Some(a) if a.primary_dir == Some(AddonDirType::Workshop) && !a.enabled => {
                skipped_workshop_only.push(aid.clone());
            }
            _ => {}
        }
    }

    for addon in &addons {
        if addon.install_state != InstallState::Installed {
            continue;
        }
        if addon.primary_dir != Some(AddonDirType::Loading) {
            if desired.contains(&addon.id) != addon.enabled {
                skipped_workshop_only.push(addon.id.clone());
            }
            continue;
        }

        let should_enable = desired.contains(&addon.id);
        if should_enable && !addon.enabled {
            to_enable.push(addon.id.clone());
        } else if !should_enable && addon.enabled {
            to_disable.push(addon.id.clone());
        }
    }

    let toggles = to_enable
        .iter()
        .map(|id| (id, true))
        .chain(to_disable.iter().map(|id| (id, false)));
    let batch = toggle_addons(db, config, image_cache_dir, toggles, false)?;
    let mut enabled_count = 0;
    let mut disabled_count = 0;
    for item in &batch.items {
        if item.outcome == AddonOperationOutcome::Applied {
            if desired.contains(&item.addon_id) {
                enabled_count += 1;
            } else {
                disabled_count += 1;
            }
        }
    }

    skipped_workshop_only.sort();
    skipped_workshop_only.dedup();

    Ok(ProfileApplyReport {
        profile_id: profile_id.clone(),
        enabled_count,
        disabled_count,
        skipped_workshop_only,
        missing_addons,
        batch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use l4a_protocol::{MetadataSource, WorkshopDependency, WorkshopFileType, WorkshopItemStatus};

    fn make_addon(
        id: &str,
        wid: Option<u64>,
        file_name: &str,
        addon_title: Option<&str>,
        author: Option<&str>,
        categories: &[&str],
        map_codes: &[&str],
    ) -> AddonSummary {
        let mut info = BTreeMap::new();
        if let Some(t) = addon_title {
            info.insert("addontitle".into(), t.into());
        }
        if let Some(a) = author {
            info.insert("addonauthor".into(), a.into());
        }
        AddonSummary {
            id: AddonId(id.into()),
            workshop_id: wid.and_then(|w| WorkshopId::new(w).ok()),
            title: addon_title
                .map(str::to_string)
                .unwrap_or_else(|| file_name.trim_end_matches(".vpk").to_string()),
            addon_title: addon_title.map(str::to_string),
            workshop_title: None,
            author: author.map(str::to_string),
            version: Some("1.0".into()),
            install_state: InstallState::Installed,
            primary_dir: Some(AddonDirType::Loading),
            enabled: true,
            current_file_name: Some(file_name.into()),
            current_path: Some(format!("/addons/{file_name}")),
            content_hash: Some(format!("hash_{id}")),
            file_size: 1024,
            files_count: 10,
            cover_image: None,
            categories: categories.iter().map(|c| (*c).to_string()).collect(),
            maps: map_codes
                .iter()
                .map(|c| MapEntry {
                    code: (*c).into(),
                    name: (*c).into(),
                    image_path: None,
                    image_hint: None,
                })
                .collect(),
            has_dummy_bypass: false,
            remote_status: WorkshopItemStatus::Unknown,
            local_time_updated: None,
            remote_time_updated: None,
            update_available: false,
            addon_info: info,
        }
    }

    fn make_ws_snapshot(
        wid: u64,
        title: &str,
        creator_id: &str,
        tags: &[&str],
        deps: &[u64],
    ) -> WorkshopItemRecord {
        WorkshopItemRecord {
            workshop_id: WorkshopId::new(wid).unwrap(),
            file_type: WorkshopFileType::Item,
            status: WorkshopItemStatus::Active,
            title: title.into(),
            description: Some("Campaign multi-part workshop item description".into()),
            creator_steam_id: Some(creator_id.into()),
            creator_name: Some(format!("Creator_{creator_id}")),
            preview_url: None,
            previews: Vec::new(),
            file_url: None,
            file_size: Some(2048),
            time_created: Some(1_700_000_000),
            time_updated: Some(1_700_000_100),
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            dependencies: deps
                .iter()
                .map(|d| WorkshopDependency {
                    workshop_id: WorkshopId::new(*d).unwrap(),
                    title: None,
                    required: true,
                })
                .collect(),
            collection_children: Vec::new(),
            subscriptions: None,
            favorites: None,
            source: MetadataSource::SteamworksSdk,
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn excludes_yama_missing_texture_fix_and_survivor_expression_dependencies() {
        let db = LocalDb::open_in_memory().unwrap();

        // Yama Part 1 + Part 2 (same author) + third-party "地图缺失材质修复" & "Yama 缺失材质修复"
        db.upsert_addon(&make_addon(
            "ws:101",
            Some(101),
            "[101] Yama_Part1.vpk",
            Some("Yama"),
            Some("Kuroda"),
            &["Campaign"],
            &["l4d_yama_1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:102",
            Some(102),
            "[102] Yama_Part2.vpk",
            None, // Part 2 VPK missing addoninfo.txt!
            None,
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:103",
            Some(103),
            "[103] Map_Missing_Texture_Fix.vpk",
            Some("地图缺失材质修复"),
            Some("Fixer"),
            &["Campaign"],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:104",
            Some(104),
            "[104] Yama_Texture_Fix.vpk",
            Some("Yama 缺失材质修复"),
            Some("Kuroda"),
            &["Campaign"],
            &[],
        ))
        .unwrap();

        // Survivor facial expression depending on survivor base model -> must NEVER bundle
        db.upsert_addon(&make_addon(
            "ws:201",
            Some(201),
            "201.vpk",
            Some("Zoey Facial Expressions"),
            Some("ModelerA"),
            &["Survivor"],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:202",
            Some(202),
            "202.vpk",
            Some("Zoey Base Model"),
            Some("ModelerA"),
            &["Survivor"],
            &[],
        ))
        .unwrap();

        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                101,
                "Yama (Part 1 - Maps)",
                "76561198000000001",
                &["Campaigns"],
                &[102],
            ),
            true,
        )
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                102,
                "Yama (Part 2 - Materials)",
                "76561198000000001",
                &["Campaigns"],
                &[],
            ),
            true,
        )
        .unwrap();
        // Fix mods declare Yama (101) as required dependency on Workshop
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                103,
                "地图缺失材质修复",
                "76561198000000099",
                &["Campaigns"],
                &[101],
            ),
            true,
        )
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                104,
                "Yama 缺失材质修复",
                "76561198000000001",
                &["Campaigns"],
                &[101],
            ),
            true,
        )
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                201,
                "Zoey Facial Expressions",
                "76561198000000055",
                &["Survivors"],
                &[202],
            ),
            true,
        )
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                202,
                "Zoey Base Model",
                "76561198000000055",
                &["Survivors"],
                &[],
            ),
            true,
        )
        .unwrap();

        let bundles = rebuild_auto_bundles(&db).unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].name, "Yama");
        assert_eq!(bundles[0].kind, BundleKind::DependencyGraph);
        assert_eq!(
            bundles[0].member_addon_ids,
            vec![AddonId("ws:101".into()), AddonId("ws:102".into())]
        );
    }

    #[test]
    fn groups_local_attachment_campaign_parts_and_ignores_sdk_boilerplate_templates() {
        let db = LocalDb::open_in_memory().unwrap();

        // Case 1: Local 3-part campaign where all VPKs copied identical addoninfo.txt ("寂静岭")
        let mut sh1 = make_addon(
            "hash:sh1",
            None,
            "silenthill_part1.vpk",
            Some("寂静岭"),
            Some("MapperSH"),
            &["Campaign"],
            &["sh_m1", "sh_m2"],
        );
        sh1.addon_info.insert(
            "addondescription".into(),
            "Classic Silent Hill custom campaign".into(),
        );
        let mut sh2 = make_addon(
            "hash:sh2",
            None,
            "silenthill_part2.vpk",
            Some("寂静岭"),
            Some("MapperSH"),
            &[], // Part 2 only has models/materials!
            &[],
        );
        sh2.addon_info.insert(
            "addondescription".into(),
            "Classic Silent Hill custom campaign".into(),
        );
        let sh3 = make_addon(
            "hash:sh3",
            None,
            "silenthill_part3.vpk",
            None, // Part 3 has NO addoninfo.txt at all!
            None,
            &[],
            &[],
        );
        db.upsert_addon(&sh1).unwrap();
        db.upsert_addon(&sh2).unwrap();
        db.upsert_addon(&sh3).unwrap();

        // Case 2: Versioned root + Part 1 + Part 2 (v1 广西-南宁 regression test)
        db.upsert_addon(&make_addon(
            "hash:nn_root",
            None,
            "nn_main.vpk",
            Some("广西-南宁 V1.15.1"),
            Some("HerobrineAce"),
            &["Campaign", "Map"],
            &["nn_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "hash:nn_p1",
            None,
            "nn_1.vpk",
            Some("广西-南宁 V1.15.1 Part 1"),
            Some("HerobrineAce"),
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "hash:nn_p2",
            None,
            "nn_2.vpk",
            Some("广西-南宁 V1.15.1 Part 2"),
            Some("HerobrineAce"),
            &[],
            &[],
        ))
        .unwrap();

        // Case 3: Two unrelated maps leaving Valve SDK sample "Dead Line" / "AUTHOR_NAME" untouched -> must NOT group!
        let mut sdk_a = make_addon(
            "hash:sdk_a",
            None,
            "alpha_custom.vpk",
            Some("Dead Line"),
            Some("AUTHOR_NAME"),
            &["Campaign"],
            &["alpha_m1"],
        );
        sdk_a.addon_info.insert(
            "addondescription".into(),
            "Same copied SDK description text".into(),
        );
        let mut sdk_b = make_addon(
            "hash:sdk_b",
            None,
            "omega_custom.vpk",
            Some("Dead Line"),
            Some("AUTHOR_NAME"),
            &["Campaign"],
            &["omega_m1"],
        );
        sdk_b.addon_info.insert(
            "addondescription".into(),
            "Same copied SDK description text".into(),
        );
        db.upsert_addon(&sdk_a).unwrap();
        db.upsert_addon(&sdk_b).unwrap();

        let bundles = rebuild_auto_bundles(&db).unwrap();
        assert_eq!(bundles.len(), 2);

        let nn = bundles
            .iter()
            .find(|b| b.name == "广西-南宁")
            .expect("广西-南宁 bundle");
        assert_eq!(
            nn.member_addon_ids,
            vec![
                AddonId("hash:nn_root".into()),
                AddonId("hash:nn_p1".into()),
                AddonId("hash:nn_p2".into()),
            ]
        );

        let sh = bundles
            .iter()
            .find(|b| b.name == "寂静岭")
            .expect("寂静岭 bundle");
        assert_eq!(
            sh.member_addon_ids,
            vec![
                AddonId("hash:sh1".into()),
                AddonId("hash:sh2".into()),
                AddonId("hash:sh3".into()),
            ]
        );
    }

    #[test]
    fn groups_identical_local_campaign_packages_with_different_filenames() {
        let db = LocalDb::open_in_memory().unwrap();

        // Local campaign split into arbitrary filenames (a.vpk, b.vpk) with identical addoninfo
        let mut a = make_addon(
            "hash:loc_a",
            None,
            "archive_main.vpk",
            Some("Bloody Moors"),
            Some(" MoorsTeam "),
            &["Campaign"],
            &["bm_m1", "bm_m2"],
        );
        a.addon_info.insert(
            "addondescription".into(),
            "Five-chapter gothic moorland survival campaign.".into(),
        );
        let mut b = make_addon(
            "hash:loc_b",
            None,
            "extra_pack.vpk",
            Some("Bloody Moors"),
            Some("MoorsTeam"),
            &[],
            &[],
        );
        b.addon_info.insert(
            "addondescription".into(),
            "Five-chapter gothic moorland survival campaign.".into(),
        );
        db.upsert_addon(&a).unwrap();
        db.upsert_addon(&b).unwrap();

        let bundles = rebuild_auto_bundles(&db).unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].name, "Bloody Moors");
        assert_eq!(
            bundles[0].member_addon_ids,
            vec![AddonId("hash:loc_a".into()), AddonId("hash:loc_b".into())]
        );
    }

    #[test]
    fn covers_compound_assets_trailing_versions_cjk_roots_and_fix_annotated_campaigns() {
        let db = LocalDb::open_in_memory().unwrap();

        // 1. Compound asset qualifier `(Models & Materials)` + `Weapon`/`Infected` content flags on Part 2
        db.upsert_addon(&make_addon(
            "ws:5001",
            Some(5001),
            "[5001] Yama_Maps.vpk",
            Some("Yama (Maps)"),
            Some("Kuroda"),
            &["Campaign", "Map"],
            &["l4d_yama_1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:5002",
            Some(5002),
            "[5002] Yama_Assets.vpk",
            Some("Yama (Models & Materials)"),
            Some("Kuroda"),
            &["Infected", "Weapon"],
            &[],
        ))
        .unwrap();

        // 2. Trailing version after `Part N` + English number words `Chapter One` / `Chapter Two`
        db.upsert_addon(&make_addon(
            "hash:ch1",
            None,
            "pripyat_ch1.vpk",
            Some("Pripyat: Chapter One v2.1"),
            Some("ZoneTeam"),
            &["Campaign", "Map"],
            &["pr_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "hash:ch2",
            None,
            "pripyat_ch2.vpk",
            Some("Pripyat: Chapter Two v2.1"),
            Some("ZoneTeam"),
            &["Campaign", "Map"],
            &["pr_m2"],
        ))
        .unwrap();

        // 3. 2-char CJK campaign (`樱花_1.vpk` + `樱花_2.vpk`) with `汉化修复版` in shared campaign title + hotfix version split
        let mut yh1 = make_addon(
            "hash:yh1",
            None,
            "樱花_1.vpk",
            Some("樱花 (汉化修复版)"),
            Some("SakuraDev"),
            &["Campaign", "Map"],
            &["yh_m1"],
        );
        yh1.version = Some("1.2".into());
        let mut yh2 = make_addon(
            "hash:yh2",
            None,
            "樱花_2.vpk",
            Some("樱花 (汉化修复版)"),
            Some("SakuraDev"),
            &[],
            &[],
        );
        yh2.version = Some("1.0".into());
        db.upsert_addon(&yh1).unwrap();
        db.upsert_addon(&yh2).unwrap();

        // 4. Generic standalone fix titles (`地图缺失材质修复`) must still NEVER merge!
        let mut fix_a = make_addon(
            "hash:fix_a",
            None,
            "missing_tex_a.vpk",
            Some("地图缺失材质修复"),
            Some("Fixer"),
            &["Campaign"],
            &["fix_m1"],
        );
        fix_a
            .addon_info
            .insert("addondescription".into(), "Same generic fix desc".into());
        let mut fix_b = make_addon(
            "hash:fix_b",
            None,
            "missing_tex_b.vpk",
            Some("地图缺失材质修复"),
            Some("Fixer"),
            &[],
            &[],
        );
        fix_b
            .addon_info
            .insert("addondescription".into(), "Same generic fix desc".into());
        db.upsert_addon(&fix_a).unwrap();
        db.upsert_addon(&fix_b).unwrap();

        let bundles = rebuild_auto_bundles(&db).unwrap();
        assert_eq!(bundles.len(), 3);
        let names: BTreeSet<String> = bundles.iter().map(|b| b.name.clone()).collect();
        assert!(names.contains("Yama"));
        assert!(names.contains("Pripyat"));
        assert!(names.contains("樱花 (汉化修复版)"));
    }
}
