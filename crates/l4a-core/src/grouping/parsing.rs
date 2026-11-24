use super::candidate::PartParse;
use crate::ops::strip_suffix_ascii_case;
use regex::Regex;
use std::sync::OnceLock;

pub(super) fn root_key_len_valid(key: &str) -> bool {
    key.chars().count() >= 3 || key.chars().filter(|c| !c.is_ascii()).count() >= 2
}

pub(super) fn parse_part_from_title(raw: &str, allow_bare_numeric: bool) -> Option<PartParse> {
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

    // 1. 括号/方括号包裹的分包后缀：`Yama (Part 1 - Maps)`、`Yama (Models & Materials)`、`Campaign [1/3]`、`战役 (第1部分)`
    let re_bracket = RE_BRACKET_PART.get_or_init(|| {
        Regex::new(
            r"(?i)^(.*?)[\s\-_#:]*[\(\[\{]\s*(?:(?:part|pt|partie|pts|pack|vol|volume|chapter|ch|ep|episode)\s*(\d+|[ivx]+|one|two|three|four|five|six|seven|eight|nine|ten)(?:\s*[/\-]\s*\d+)?(?:\s*[-:–]\s*[^\)\]\}]*)?|(\d+)\s*/\s*\d+|(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|props|media)(?:\s*(?:&|and|/|\+|、|与|及)\s*(?:textures?|materials?|models?|sounds?|audio|music|assets?(?:\s+pack)?|resources?(?:\s+pack)?|maps?(?:\s+pack)?|main|props|media))*|分[包卷]\s*(\d*)|第\s*(\d+|[一二三四五六七八九十]+)\s*(?:部分|[部章分卷包])|(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包)(?:\s*(?:&|/|、|与|及)\s*(?:材质包?|贴图包?|模型包?|音效包?|音频包?|资源包?|地图包?|主包))*)\s*[\)\]\}]\s*$",
        )
        .unwrap()
    });
    if let Some(caps) = re_bracket.captures(trimmed) {
        let raw_root = caps.get(1).map_or("", |m| m.as_str());
        let idx_str = caps
            .get(2)
            .or_else(|| caps.get(3))
            .or_else(|| caps.get(4))
            .or_else(|| caps.get(5))
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
            r"(?i)^(.*?)[\s\-_#:：]*(?:分[包卷]\s*(\d+|[一二三四五六七八九十]+)|第\s*(\d+|[一二三四五六七八九十]+)\s*(?:部分|[部章分卷包])|((?:主包|地图包|材质包|贴图包|模型包|音效包|音频包|资源包)(?:\s*(?:&|/|、|与|及)\s*(?:主包|地图包|材质包|贴图包|模型包|音效包|音频包|资源包))*))$",
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
        let base_rank: u32 = match caps
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
            .map(|m| m.as_str().parse::<u32>().unwrap_or(10_000).min(10_000))
            .unwrap_or(0);
        if let Some(parsed) = finalize_part_parse(raw_root, base_rank.saturating_add(offset), false)
        {
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
    if s.is_empty() {
        return None;
    }
    if s.chars().all(|c| c.is_ascii_digit()) {
        return Some(s.parse::<u32>().unwrap_or(10_000).min(10_000));
    }
    match s.as_str() {
        "one" | "一" => Some(1),
        "two" | "二" => Some(2),
        "three" | "三" => Some(3),
        "four" | "四" => Some(4),
        "five" | "五" => Some(5),
        "six" | "六" => Some(6),
        "seven" | "七" => Some(7),
        "eight" | "八" => Some(8),
        "nine" | "九" => Some(9),
        "ten" | "十" => Some(10),
        _ => parse_roman_part_number(&s),
    }
}

fn parse_roman_part_number(raw: &str) -> Option<u32> {
    let mut total = 0i64;
    let mut largest_right = 0;
    for digit in raw.bytes().rev() {
        let value = match digit {
            b'i' => 1,
            b'v' => 5,
            b'x' => 10,
            _ => return None,
        };
        if value < largest_right {
            total -= value;
        } else {
            total += value;
            largest_right = value;
        }
    }
    (total > 0).then_some(total.min(10_000) as u32)
}

pub(super) fn extract_explicit_release_key(raw: &str) -> Option<String> {
    static RE_REL: OnceLock<Regex> = OnceLock::new();
    let re = RE_REL
        .get_or_init(|| Regex::new(r"(?i)(?:v|ver|version)\.?\s*(\d+(?:\.\d+)*[a-z]?)").unwrap());
    let mut last_ver = None;
    for caps in re.captures_iter(raw) {
        let (Some(full), Some(ver_match)) = (caps.get(0), caps.get(1)) else {
            continue;
        };
        let left_ok = raw[..full.start()]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        let right_ok = raw[full.end()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '.');
        if left_ok && right_ok {
            last_ver = Some(normalize_version_token(ver_match.as_str()));
        }
    }
    last_ver
}

pub(super) fn normalize_version_token(ver: &str) -> String {
    let trimmed = ver
        .trim()
        .trim_start_matches(['v', 'V'])
        .trim()
        .to_ascii_lowercase();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.') {
        let mut parts: Vec<&str> = trimmed.split('.').filter(|p| !p.is_empty()).collect();
        while parts.len() > 1 && parts.last() == Some(&"0") {
            parts.pop();
        }
        if !parts.is_empty() {
            return parts.join(".");
        }
    }
    trimmed
}

pub(super) fn strip_trailing_version(raw: &str) -> String {
    static RE_VER: OnceLock<Regex> = OnceLock::new();
    let re = RE_VER.get_or_init(|| {
        Regex::new(r"(?i)[\s\-_]*(?:\(\s*)?(?:v|ver|version)\.?\s*\d+(?:\.\d+)*[a-z]?(?:\s*\))?$")
            .unwrap()
    });
    let stripped = re.replace(raw.trim(), "");
    stripped.into_owned()
}

pub(super) fn strip_filename_decorations(file_name: &str) -> String {
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

pub(super) fn strip_texture_group_root_key(title_key: &str) -> Option<String> {
    let stripped = title_key
        .strip_suffix(" textures")
        .or_else(|| title_key.strip_suffix(" texture"))
        .or_else(|| title_key.strip_suffix(" materials"))
        .map(str::trim)?;
    root_key_len_valid(stripped).then(|| stripped.to_string())
}

pub(super) fn normalize_comparison_text(value: &str) -> String {
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

pub(super) fn normalize_non_placeholder_author(raw: &str) -> Option<String> {
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

pub(super) fn is_sdk_boilerplate_title(raw: &str) -> bool {
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

pub(super) fn is_boilerplate_description(norm_desc: &str) -> bool {
    norm_desc.contains("dead line add on")
        || norm_desc.contains("short description of your add on")
        || norm_desc.contains("enter a description")
}

pub(super) fn is_generic_file_stem(root_key: &str) -> bool {
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

pub(super) fn contains_fix_marker(text: &str) -> bool {
    static RE_FIX_EN: OnceLock<Regex> = OnceLock::new();
    let norm = normalize_comparison_text(text);
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
        if norm.contains(kw) {
            return true;
        }
    }
    let re = RE_FIX_EN.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:fix|fixed|fixes|hotfix|bugfix|patch|patched|missing\s+textures?|missing\s+materials?|nav\s*fix|skybox\s+fix|translation|localization|subtitles?)\b",
        )
        .unwrap()
    });
    re.is_match(&norm)
}

/// 判断标题是否本身就是一个独立的通用修复补丁名称（如 `地图缺失材质修复`、`Missing Texture Fix`），而非带有 `(汉化修复版)` / `(Fixed)` 的真实战役名。
pub(super) fn is_standalone_fix_title(raw: &str) -> bool {
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

pub(super) fn push_unique_str(vec: &mut Vec<String>, val: &str) {
    if !val.is_empty() && !vec.iter().any(|existing| existing == val) {
        vec.push(val.to_string());
    }
}

pub(super) fn clean_root_title(raw: &str) -> String {
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
