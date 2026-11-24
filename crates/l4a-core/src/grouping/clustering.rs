use super::candidate::BundleCandidate;
use super::parsing::{
    clean_root_title, is_generic_file_stem, normalize_comparison_text, root_key_len_valid,
    strip_trailing_version,
};
use super::partition::{
    authors_compatible, explicit_releases_compatible, group_authors_consistent,
    group_releases_consistent,
};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub(super) fn is_valid_campaign_dependency_edge(u: &BundleCandidate, v: &BundleCandidate) -> bool {
    if !u.eligible_for_campaign_bundle() || !v.eligible_for_campaign_bundle() {
        return false;
    }
    if !authors_compatible(u, v) || !explicit_releases_compatible(u, v) {
        return false;
    }
    let Some(shared_root) = shared_campaign_root_key(u, v) else {
        return false;
    };
    !u.has_standalone_fix_marker_outside(shared_root)
        && !v.has_standalone_fix_marker_outside(shared_root)
}

fn shared_campaign_root_key<'a>(u: &'a BundleCandidate, v: &'a BundleCandidate) -> Option<&'a str> {
    for uk in &u.metadata_root_keys {
        if root_key_len_valid(uk) && v.metadata_root_keys.iter().any(|vk| vk == uk) {
            return Some(uk);
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
                return Some(uf);
            }
        }
    }
    None
}

fn shared_part_prefix_key<'a>(a: &'a str, b: &'a str) -> Option<&'a str> {
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if !root_key_len_valid(shorter) {
        return None;
    }
    let rest = longer.strip_prefix(shorter)?.trim();
    if rest.is_empty() {
        return Some(shorter);
    }
    let trimmed_rest = rest.trim_start_matches([':', '-', '_', '/', '|', '(', '[', ' ']);
    is_recognized_subpackage_qualifier(trimmed_rest).then_some(shorter)
}

fn is_recognized_subpackage_qualifier(s: &str) -> bool {
    static RE_QUAL: OnceLock<Regex> = OnceLock::new();
    let re = RE_QUAL.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:(?:part|pt|partie|pts|pack|vol|volume|chapter|ch|ep|episode)\s*(?:\d+|[ivxldcm]+|one|two|three|four|five|six|seven|eight|nine|ten)?(?:\s*[/\-]\s*\d+)?|(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|base|media|props|content)(?:\s*(?:&|and|/|\+|、|与|及)\s*(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|base|media|props|content))*|分[包卷]\s*\d*|第\s*[\d一二三四五六七八九十]+\s*(?:部分|[部章分卷包])|(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包)(?:\s*(?:&|/|、|与|及)\s*(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包))*)[\s\)\]]*$",
        )
        .unwrap()
    });
    re.is_match(s.trim())
}

pub(super) fn is_valid_campaign_group(members: &[&BundleCandidate]) -> bool {
    if members.len() < 2 {
        return false;
    }
    if members.iter().any(|m| !m.eligible_for_campaign_bundle()) {
        return false;
    }
    if !members.iter().any(|m| m.has_campaign_signal) {
        return false;
    }
    group_authors_consistent(members) && group_releases_consistent(members, false)
}

pub(super) fn metadata_titles_compatible(members: &[&BundleCandidate]) -> bool {
    let titled: Vec<&BundleCandidate> = members
        .iter()
        .copied()
        .filter(|m| !m.metadata_root_keys.is_empty())
        .collect();
    if titled.len() <= 1 {
        return true;
    }
    // 每条依赖边可能匹配不同的标题。整组有标题的成员需要共享同一个词根。
    titled
        .iter()
        .flat_map(|m| &m.metadata_root_keys)
        .any(|root| {
            titled.iter().all(|member| {
                member
                    .metadata_root_keys
                    .iter()
                    .any(|key| shared_part_prefix_key(root, key) == Some(root.as_str()))
            })
        })
}

/// 若同组有元数据标题的成员中，仅部分成员含有「修复/补丁/汉化」标记而其他成员没有，判定为独立补丁混入并拒绝。
pub(super) fn group_has_asymmetric_fix_marker(members: &[&BundleCandidate]) -> bool {
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

pub(super) fn has_conflicting_versions(members: &[&BundleCandidate]) -> bool {
    let versions: HashSet<&str> = members
        .iter()
        .filter_map(|m| m.version_key.as_deref())
        .collect();
    versions.len() > 1
}

pub(super) fn has_complementary_map_split(members: &[&BundleCandidate]) -> bool {
    let mut seen = HashSet::new();
    for member in members {
        let codes: HashSet<_> = member
            .addon
            .maps
            .iter()
            .map(|map| map.code.to_ascii_lowercase())
            .collect();
        if !seen.is_disjoint(&codes) {
            return false;
        }
        seen.extend(codes);
    }
    members.len() >= 2 && !seen.is_empty()
}

/// 校验本地附件/工坊同名战役包是否确实属于同一套战役的分包（避免两个碰巧同名的独立地图误合并）。
pub(super) fn identical_title_group_corroborated(members: &[&BundleCandidate]) -> bool {
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

pub(super) fn sort_bundle_candidates(members: &mut [&BundleCandidate]) {
    members.sort_by(|a, b| {
        a.sort_rank()
            .cmp(&b.sort_rank())
            .then_with(|| a.addon.id.cmp(&b.addon.id))
    });
}

/// 统一生成纯净的战役整合包标题，绝不混入裸文件名或 `[workshop_id]` / `Part 1` 尾巴。
pub(super) fn derive_bundle_name(members: &[&BundleCandidate]) -> String {
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
                if (disp.len(), disp.as_str()) < (entry.1.len(), entry.1.as_str()) {
                    entry.1 = disp.clone();
                }
            }
        }
        if let Some((_, (_, best_disp))) = root_counts
            .into_iter()
            .filter(|(_, (count, _))| *count >= 2)
            .min_by(
                |(key_a, (count_a, display_a)), (key_b, (count_b, display_b))| {
                    count_b
                        .cmp(count_a)
                        .then_with(|| display_a.len().cmp(&display_b.len()))
                        .then_with(|| key_a.cmp(key_b))
                },
            )
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
