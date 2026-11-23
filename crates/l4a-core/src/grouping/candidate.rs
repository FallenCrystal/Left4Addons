use super::parsing::{
    clean_root_title, contains_fix_marker, extract_explicit_release_key,
    is_boilerplate_description, is_sdk_boilerplate_title, is_standalone_fix_title,
    normalize_comparison_text, normalize_non_placeholder_author, normalize_version_token,
    parse_part_from_title, push_unique_str, root_key_len_valid, strip_filename_decorations,
    strip_trailing_version,
};
use l4a_protocol::{AddonSummary, WorkshopItemRecord};

#[derive(Debug, Clone)]
pub(super) struct PartParse {
    pub(super) root_display: String,
    pub(super) root_key: String,
    pub(super) order_rank: u32,
    pub(super) bare_numeric: bool,
}

#[derive(Debug, Clone)]
pub(super) struct BundleCandidate {
    pub(super) addon: AddonSummary,
    pub(super) has_campaign_signal: bool,
    pub(super) exclusively_non_campaign: bool,
    pub(super) is_pure_survivor_or_ui: bool,
    pub(super) has_explicit_part_role: bool,
    pub(super) steam_creator_id: Option<String>,
    pub(super) author_name_key: Option<String>,
    pub(super) description_key: Option<String>,
    pub(super) tagline_key: Option<String>,
    pub(super) url_key: Option<String>,
    pub(super) version_key: Option<String>,
    pub(super) title_release_key: Option<String>,
    pub(super) file_release_key: Option<String>,
    pub(super) clean_addon_title: Option<String>,
    pub(super) clean_workshop_title: Option<String>,
    pub(super) metadata_part: Option<PartParse>,
    pub(super) metadata_root_keys: Vec<String>,
    pub(super) metadata_display_roots: Vec<String>,
    pub(super) file_stem: Option<String>,
    pub(super) file_clean_root_key: Option<String>,
    pub(super) file_part: Option<PartParse>,
    pub(super) identical_group_title_key: Option<String>,
    pub(super) is_boilerplate_title: bool,
    pub(super) is_standalone_fix_only_title: bool,
    pub(super) has_any_fix_marker: bool,
    pub(super) has_metadata_fix_marker: bool,
    pub(super) all_titles_lower: Vec<String>,
}

impl BundleCandidate {
    pub(super) fn from_addon(addon: &AddonSummary, snap: Option<&WorkshopItemRecord>) -> Self {
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
            .map(normalize_version_token);

        let clean_addon_title = addon.addon_title.clone().or_else(|| {
            crate::db::repo::clean_addon_title(
                addon.addon_info.get("addontitle").map(String::as_str),
            )
        });
        let clean_workshop_title = addon.workshop_title.clone().or_else(|| {
            snap.and_then(|s| crate::db::repo::clean_workshop_title(Some(s.title.as_str())))
        });

        let title_release_key = clean_addon_title
            .as_deref()
            .and_then(extract_explicit_release_key)
            .or_else(|| {
                clean_workshop_title
                    .as_deref()
                    .and_then(extract_explicit_release_key)
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

        let file_release_key = file_stem.as_deref().and_then(extract_explicit_release_key);

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
            title_release_key,
            file_release_key,
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

    pub(super) fn eligible_for_campaign_bundle(&self) -> bool {
        if !self.exclusively_non_campaign {
            return true;
        }
        // 允许战役的 Part 2/资源包同时携带自定义感染者或武器标记，但严禁纯人物模型/表情或 UI 模组混入
        self.has_explicit_part_role && !self.is_pure_survivor_or_ui
    }

    pub(super) fn matches_metadata_root(&self, root_key: &str) -> bool {
        self.metadata_root_keys.iter().any(|k| k == root_key)
    }

    pub(super) fn effective_release_key(&self, prefer_file: bool) -> Option<&str> {
        if prefer_file {
            self.file_release_key
                .as_deref()
                .or(self.title_release_key.as_deref())
        } else {
            self.title_release_key
                .as_deref()
                .or(self.file_release_key.as_deref())
        }
    }

    pub(super) fn part_rank_for(&self, prefer_file: bool) -> Option<u32> {
        if prefer_file {
            self.file_part
                .as_ref()
                .map(|p| p.order_rank)
                .or_else(|| self.metadata_part.as_ref().map(|p| p.order_rank))
        } else {
            self.metadata_part
                .as_ref()
                .map(|p| p.order_rank)
                .or_else(|| self.file_part.as_ref().map(|p| p.order_rank))
        }
    }

    /// 检查除共享战役词根外的附加后缀中是否含有「修复 / 补丁 / 缺失材质 / 汉化」等独立补丁标记。
    pub(super) fn has_standalone_fix_marker_outside(&self, shared_root_key: &str) -> bool {
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
    pub(super) fn has_file_fix_marker_outside(&self, file_root_key: &str) -> bool {
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

    pub(super) fn sort_rank(&self) -> (u32, u8, String) {
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
