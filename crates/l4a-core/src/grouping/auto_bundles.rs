use super::candidate::BundleCandidate;
use super::clustering::{
    derive_bundle_name, group_has_asymmetric_fix_marker, has_complementary_map_split,
    has_conflicting_versions, identical_title_group_corroborated,
    is_valid_campaign_dependency_edge, is_valid_campaign_group, metadata_titles_compatible,
    sort_bundle_candidates,
};
use super::parsing::{is_generic_file_stem, strip_texture_group_root_key};
use super::partition::{
    group_author_signature, group_authors_consistent, group_release_signature,
    has_conflicting_duplicate_parts, partition_by_author_and_release,
    split_duplicate_parts_across_versions,
};
use crate::db::LocalDb;
use crate::error::CoreError;
use l4a_protocol::{
    AddonId, AddonSummary, BundleId, BundleKind, BundleRecord, InstallState, MapEntry, WorkshopId,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

/// 按四层清晰模型管理：
/// 1. `Bundle`: 战役/地图多包物理整合（支持 `locked` 保护与跨分包地图/封面聚合）
/// 2. `TrackedCollection`: 上游工坊合集追踪
/// 3. `UserGroup`: 多对多自定义分类/标签
/// 4. `LoadoutProfile`: 一键启用预设
pub fn rebuild_auto_bundles(db: &LocalDb) -> Result<Vec<BundleRecord>, CoreError> {
    let existing = db.list_bundles()?;
    let mut locked_member_ids: HashSet<AddonId> = HashSet::new();
    let mut reserved_bundle_ids: HashSet<BundleId> = HashSet::new();
    for b in &existing {
        if b.locked || b.kind == BundleKind::Manual {
            reserved_bundle_ids.insert(b.id.clone());
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
            .filter(|id| !assigned.contains(*id))
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
        let bundle_id = allocate_auto_bundle_id(
            format!("bundle_dep_{primary_wid}"),
            &member_ids,
            &mut reserved_bundle_ids,
        );
        let bundle = assemble_bundle(
            bundle_id,
            bundle_name,
            BundleKind::DependencyGraph,
            false,
            member_ids.clone(),
            &addon_map,
        );
        if db.save_auto_bundle(&bundle)? {
            for aid in member_ids {
                assigned.insert(aid);
            }
        }
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

        for (root_key, seeded_members) in by_root {
            let active_seeded: Vec<&BundleCandidate> = seeded_members
                .into_iter()
                .filter(|c| !assigned.contains(&c.addon.id))
                .collect();
            if active_seeded.is_empty() {
                continue;
            }
            let seeded_ids: HashSet<&AddonId> = active_seeded.iter().map(|c| &c.addon.id).collect();
            let mut combined_for_root = active_seeded;
            // 同步纳入同名战役主包（例如 `City Escape` 或 `广西-南宁 V1.15.1`），由对称分簇器统一消解作者与发布版通配，杜绝跨簇先到先得抢占
            for cand in &pool {
                if assigned.contains(&cand.addon.id) || seeded_ids.contains(&cand.addon.id) {
                    continue;
                }
                if cand.matches_metadata_root(&root_key)
                    && !cand.has_standalone_fix_marker_outside(&root_key)
                {
                    combined_for_root.push(*cand);
                }
            }

            for base_cluster in partition_by_author_and_release(combined_for_root, false) {
                let prelim: Vec<&BundleCandidate> = base_cluster
                    .into_iter()
                    .filter(|c| !assigned.contains(&c.addon.id))
                    .collect();
                if prelim.is_empty() {
                    continue;
                }

                for mut members in split_duplicate_parts_across_versions(prelim, false) {
                    members.retain(|c| !assigned.contains(&c.addon.id));
                    if members.len() < 2
                        || !members.iter().any(|c| seeded_ids.contains(&c.addon.id))
                        || !is_valid_campaign_group(&members)
                        || has_conflicting_duplicate_parts(&members, false)
                    {
                        continue;
                    }
                    sort_bundle_candidates(&mut members);
                    let display_name = derive_bundle_name(&members);
                    let author_sig = group_author_signature(&members);
                    let release_sig = group_release_signature(&members);
                    let seed = if release_sig.is_empty() {
                        format!("title|{root_key}|{author_sig}")
                    } else {
                        format!("title|{root_key}|{author_sig}|{release_sig}")
                    };
                    let member_ids: Vec<AddonId> =
                        members.iter().map(|c| c.addon.id.clone()).collect();
                    if save_campaign_parts_bundle(
                        db,
                        &seed,
                        display_name,
                        member_ids.clone(),
                        &addon_map,
                        &mut reserved_bundle_ids,
                    )? {
                        for aid in member_ids {
                            assigned.insert(aid);
                        }
                    }
                }
            }
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

        for (file_root_key, seeded_members) in by_file_root {
            let active_seeded: Vec<&BundleCandidate> = seeded_members
                .into_iter()
                .filter(|c| !assigned.contains(&c.addon.id))
                .collect();
            if active_seeded.is_empty() {
                continue;
            }
            let seeded_ids: HashSet<&AddonId> = active_seeded.iter().map(|c| &c.addon.id).collect();
            let mut combined_for_root = active_seeded;
            for cand in &pool {
                if assigned.contains(&cand.addon.id) || seeded_ids.contains(&cand.addon.id) {
                    continue;
                }
                if cand.file_clean_root_key.as_deref() == Some(file_root_key.as_str())
                    && !cand.has_file_fix_marker_outside(&file_root_key)
                {
                    combined_for_root.push(*cand);
                }
            }

            for base_cluster in partition_by_author_and_release(combined_for_root, true) {
                let prelim: Vec<&BundleCandidate> = base_cluster
                    .into_iter()
                    .filter(|c| !assigned.contains(&c.addon.id))
                    .collect();
                if prelim.is_empty() {
                    continue;
                }

                for mut members in split_duplicate_parts_across_versions(prelim, true) {
                    members.retain(|c| !assigned.contains(&c.addon.id));
                    if members.len() < 2
                        || !members.iter().any(|c| seeded_ids.contains(&c.addon.id))
                        || !is_valid_campaign_group(&members)
                        || !metadata_titles_compatible(&members)
                        || group_has_asymmetric_fix_marker(&members)
                        || has_conflicting_duplicate_parts(&members, true)
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
                    let release_sig = group_release_signature(&members);
                    let seed = if release_sig.is_empty() {
                        format!("file|{file_root_key}|{author_sig}")
                    } else {
                        format!("file|{file_root_key}|{author_sig}|{release_sig}")
                    };
                    let member_ids: Vec<AddonId> =
                        members.iter().map(|c| c.addon.id.clone()).collect();
                    if save_campaign_parts_bundle(
                        db,
                        &seed,
                        display_name,
                        member_ids.clone(),
                        &addon_map,
                        &mut reserved_bundle_ids,
                    )? {
                        for aid in member_ids {
                            assigned.insert(aid);
                        }
                    }
                }
            }
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
            let Some(raw_tex_members) = by_meta_title.get(key) else {
                continue;
            };
            let tex_members: Vec<&BundleCandidate> = raw_tex_members
                .iter()
                .copied()
                .filter(|c| !assigned.contains(&c.addon.id))
                .collect();
            if tex_members.len() < 2 {
                continue;
            }
            let tex_ids: HashSet<&AddonId> = tex_members.iter().map(|c| &c.addon.id).collect();
            let mut combined_for_tex = tex_members;
            for cand in &pool {
                if assigned.contains(&cand.addon.id) || tex_ids.contains(&cand.addon.id) {
                    continue;
                }
                if cand.identical_group_title_key.as_deref() == Some(tex_root_key.as_str())
                    && !cand.is_standalone_fix_only_title
                    && !cand.has_file_fix_marker_outside(&tex_root_key)
                {
                    combined_for_tex.push(*cand);
                }
            }
            let mut matched_any_tex_cluster = false;
            for mut combined in partition_by_author_and_release(combined_for_tex, false) {
                combined.retain(|c| !assigned.contains(&c.addon.id));
                let tex_count = combined
                    .iter()
                    .filter(|c| tex_ids.contains(&c.addon.id))
                    .count();
                if tex_count >= 2
                    && combined.len() >= 3
                    && group_authors_consistent(&combined)
                    && is_valid_campaign_group(&combined)
                    && identical_title_group_corroborated(&combined)
                {
                    sort_bundle_candidates(&mut combined);
                    let display_name = derive_bundle_name(&combined);
                    let author_sig = group_author_signature(&combined);
                    let release_sig = group_release_signature(&combined);
                    let seed = if release_sig.is_empty() {
                        format!("ident_tex|{tex_root_key}|{author_sig}")
                    } else {
                        format!("ident_tex|{tex_root_key}|{author_sig}|{release_sig}")
                    };
                    let member_ids: Vec<AddonId> =
                        combined.iter().map(|c| c.addon.id.clone()).collect();
                    if save_campaign_parts_bundle(
                        db,
                        &seed,
                        display_name,
                        member_ids.clone(),
                        &addon_map,
                        &mut reserved_bundle_ids,
                    )? {
                        matched_any_tex_cluster = true;
                        for aid in member_ids {
                            assigned.insert(aid);
                        }
                    }
                }
            }
            if matched_any_tex_cluster {
                consumed_keys.insert(key.clone());
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

            // 按兼容作者与显式发布版本切分子簇（防止不同作者或独立发布版混入同一包）
            for mut cluster in partition_by_author_and_release(active, false) {
                cluster.retain(|c| !assigned.contains(&c.addon.id));
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
                let release_sig = group_release_signature(&cluster);
                let seed = if release_sig.is_empty() {
                    format!("ident|{title_key}|{author_sig}")
                } else {
                    format!("ident|{title_key}|{author_sig}|{release_sig}")
                };
                let member_ids: Vec<AddonId> = cluster.iter().map(|c| c.addon.id.clone()).collect();
                if save_campaign_parts_bundle(
                    db,
                    &seed,
                    display_name,
                    member_ids.clone(),
                    &addon_map,
                    &mut reserved_bundle_ids,
                )? {
                    for aid in member_ids {
                        assigned.insert(aid);
                    }
                }
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

fn allocate_auto_bundle_id(
    base_id: String,
    member_ids: &[AddonId],
    reserved: &mut HashSet<BundleId>,
) -> BundleId {
    let primary = BundleId(base_id.clone());
    if reserved.insert(primary.clone()) {
        return primary;
    }
    let mut sorted: Vec<&str> = member_ids.iter().map(AddonId::as_str).collect();
    sorted.sort_unstable();
    let mut hasher = Sha256::new();
    for id in sorted {
        hasher.update(id.as_bytes());
        hasher.update(b"|");
    }
    let suffix = &hex::encode(hasher.finalize())[..8];
    let fallback = BundleId(format!("{base_id}_{suffix}"));
    if reserved.insert(fallback.clone()) {
        return fallback;
    }
    let mut counter = 2u32;
    loop {
        let candidate = BundleId(format!("{base_id}_{suffix}_{counter}"));
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

fn save_campaign_parts_bundle(
    db: &LocalDb,
    seed: &str,
    display_name: String,
    member_ids: Vec<AddonId>,
    addon_map: &HashMap<AddonId, AddonSummary>,
    reserved_ids: &mut HashSet<BundleId>,
) -> Result<bool, CoreError> {
    let digest = hex::encode(Sha256::digest(seed.as_bytes()));
    let bundle_id = allocate_auto_bundle_id(
        format!("bundle_camp_{}", &digest[..12]),
        &member_ids,
        reserved_ids,
    );
    let bundle = assemble_bundle(
        bundle_id,
        display_name,
        BundleKind::CampaignParts,
        false,
        member_ids,
        addon_map,
    );
    db.save_auto_bundle(&bundle)
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
