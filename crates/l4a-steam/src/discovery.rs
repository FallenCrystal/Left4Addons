use std::fs;
use std::path::{Path, PathBuf};

/// 自动探测 Left 4 Dead 2 的 `left4dead2/addons` 目录。
pub fn detect_l4d2_addons_path() -> Option<String> {
    let steam_roots = get_steam_install_paths();
    let mut candidate_libraries = Vec::new();

    for root in &steam_roots {
        if root.exists() && root.is_dir() && !candidate_libraries.contains(root) {
            candidate_libraries.push(root.clone());
        }
    }

    for root in &steam_roots {
        let vdf_paths = [
            root.join("steamapps").join("libraryfolders.vdf"),
            root.join("config").join("libraryfolders.vdf"),
        ];

        for vdf_path in &vdf_paths {
            if let Ok(content) = fs::read_to_string(vdf_path) {
                for p in parse_library_folders_vdf(&content) {
                    if p.exists() && p.is_dir() && !candidate_libraries.contains(&p) {
                        candidate_libraries.push(p);
                    }
                }
            }
        }
    }

    for lib_dir in &candidate_libraries {
        if let Some(addons_dir) = find_l4d2_addons_in_library(lib_dir) {
            return Some(addons_dir.to_string_lossy().into_owned());
        }
    }

    None
}

fn get_steam_install_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        #[cfg(windows)]
        {
            use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
            use winreg::RegKey;

            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            if let Ok(key) = hkcu.open_subkey(r"Software\Valve\Steam") {
                if let Ok(val) = key.get_value::<String, _>("SteamPath") {
                    paths.push(PathBuf::from(val));
                }
                if let Ok(val) = key.get_value::<String, _>("InstallPath") {
                    paths.push(PathBuf::from(val));
                }
            }
            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
            if let Ok(key) = hklm.open_subkey(r"SOFTWARE\WOW6432Node\Valve\Steam") {
                if let Ok(val) = key.get_value::<String, _>("InstallPath") {
                    paths.push(PathBuf::from(val));
                }
            }
            if let Ok(key) = hklm.open_subkey(r"SOFTWARE\Valve\Steam") {
                if let Ok(val) = key.get_value::<String, _>("InstallPath") {
                    paths.push(PathBuf::from(val));
                }
            }
        }

        for p in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
            paths.push(PathBuf::from(p));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let home_path = PathBuf::from(home);
            paths.push(home_path.join(".steam").join("steam"));
            paths.push(home_path.join(".steam").join("root"));
            paths.push(home_path.join(".local").join("share").join("Steam"));
            paths.push(
                home_path
                    .join(".var")
                    .join("app")
                    .join("com.valvesoftware.Steam")
                    .join("data")
                    .join("Steam"),
            );
            paths.push(
                home_path
                    .join("Library")
                    .join("Application Support")
                    .join("Steam"),
            );
        }
    }

    paths
}

/// 解析 `libraryfolders.vdf`，优先返回包含 AppID `550` (L4D2) 的库目录。
pub fn parse_library_folders_vdf(content: &str) -> Vec<PathBuf> {
    let mut regular_paths = Vec::new();
    let mut prioritized_paths = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_has_550 = false;

    for line in content.lines() {
        let line_str = line.trim();
        if line_str.is_empty() || line_str.starts_with("//") {
            continue;
        }

        let tokens = extract_vdf_tokens(line_str);
        if tokens.as_slice() == ["550"]
            || (tokens.len() >= 3 && tokens[0].eq_ignore_ascii_case("apps") && tokens[1] == "550")
            || tokens.iter().any(|t| t == "550") && tokens.first().is_some_and(|t| t.eq_ignore_ascii_case("apps"))
        {
            current_has_550 = true;
        }
        let mut iter = tokens.iter();
        while let Some(raw_key) = iter.next() {
            if raw_key.eq_ignore_ascii_case("apps") {
                continue;
            }
            let Some(val) = iter.next() else {
                break;
            };
            let key = raw_key.to_ascii_lowercase();
            if key == "path" {
                if let Some(prev_path) = current_path.take() {
                    let cleaned = unescape_vdf_path(&prev_path);
                    if current_has_550 {
                        prioritized_paths.push(PathBuf::from(cleaned));
                    } else {
                        regular_paths.push(PathBuf::from(cleaned));
                    }
                }
                current_path = Some(val.clone());
                current_has_550 = false;
            } else if key == "550" {
                current_has_550 = true;
            } else if key.parse::<u32>().is_ok() && (val.contains('/') || val.contains('\\')) {
                let cleaned = unescape_vdf_path(val);
                regular_paths.push(PathBuf::from(cleaned));
            }
        }
    }

    if let Some(last_path) = current_path {
        let cleaned = unescape_vdf_path(&last_path);
        if current_has_550 {
            prioritized_paths.push(PathBuf::from(cleaned));
        } else {
            regular_paths.push(PathBuf::from(cleaned));
        }
    }

    prioritized_paths.extend(regular_paths);
    prioritized_paths
}

fn unescape_vdf_path(path_str: &str) -> String {
    path_str.replace("\\\\", "\\")
}

fn extract_vdf_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_quotes = false;
    let mut current = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' {
            if in_quotes {
                tokens.push(current.clone());
                current.clear();
                in_quotes = false;
            } else {
                in_quotes = true;
            }
        } else if in_quotes {
            if ch == '\\' {
                if let Some(&next_ch) = chars.peek() {
                    if next_ch == '\\' || next_ch == '"' {
                        current.push(next_ch);
                        chars.next();
                        continue;
                    }
                }
            }
            current.push(ch);
        }
    }

    tokens
}

pub fn find_l4d2_addons_in_library(lib_dir: &Path) -> Option<PathBuf> {
    let direct = lib_dir
        .join("steamapps")
        .join("common")
        .join("Left 4 Dead 2")
        .join("left4dead2")
        .join("addons");
    if direct.is_dir() {
        return Some(direct);
    }

    let segments = [
        "steamapps",
        "common",
        "Left 4 Dead 2",
        "left4dead2",
        "addons",
    ];
    let mut current = lib_dir.to_path_buf();

    for seg in &segments {
        if !current.is_dir() {
            return None;
        }
        let exact_child = current.join(seg);
        if exact_child.exists() {
            current = exact_child;
            continue;
        }

        let mut found = false;
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if name.eq_ignore_ascii_case(seg) {
                        current = entry.path();
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found {
            return None;
        }
    }

    current.is_dir().then_some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn modern_library_folders_vdf_prioritizes_app_550() {
        let vdf = r#"
"libraryfolders"
{
    "0"
    {
        "path" "C:\\Program Files (x86)\\Steam"
        "apps" { "220" "100" }
    }
    "1"
    {
        "path" "D:\\Games\\SteamLibrary"
        "apps" { "550" "3420120150" }
    }
}
"#;
        let paths = parse_library_folders_vdf(vdf);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from(r"D:\Games\SteamLibrary"));
        assert_eq!(paths[1], PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }

    #[test]
    fn legacy_library_folders_vdf_parses_numbered_entries() {
        let vdf = r#"
"LibraryFolders"
{
    "TimeNextStatsReport" "12345678"
    "1" "E:\\SteamLibrary"
    "2" "F:\\SteamLibrary"
}
"#;
        let paths = parse_library_folders_vdf(vdf);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from(r"E:\SteamLibrary"));
        assert_eq!(paths[1], PathBuf::from(r"F:\SteamLibrary"));
    }

    #[test]
    fn finds_addons_case_insensitively_in_library() {
        let dir = tempdir().unwrap();
        let addons = dir
            .path()
            .join("SteamApps")
            .join("common")
            .join("left 4 dead 2")
            .join("Left4Dead2")
            .join("addons");
        fs::create_dir_all(&addons).unwrap();
        assert_eq!(find_l4d2_addons_in_library(dir.path()), Some(addons));
    }
}
