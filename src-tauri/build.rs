use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    tauri_build::build();

    if let Err(err) = stage_steam_bridge() {
        println!("cargo:warning=failed to stage Steam bridge runtime files: {err}");
    }
}

fn stage_steam_bridge() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|e| e.to_string())?;
    let artifacts = if target.contains("windows") {
        SteamBridgeArtifacts {
            bridge_source_name: "l4a_steam_bridge.dll",
            bridge_staged_name: "l4a-steam-bridge.dll",
            steam_api_name: "steam_api64.dll",
        }
    } else if target.contains("linux") {
        SteamBridgeArtifacts {
            bridge_source_name: "libl4a_steam_bridge.so",
            bridge_staged_name: "libl4a-steam-bridge.so",
            steam_api_name: "libsteam_api.so",
        }
    } else if target.contains("darwin") {
        SteamBridgeArtifacts {
            bridge_source_name: "libl4a_steam_bridge.dylib",
            bridge_staged_name: "libl4a-steam-bridge.dylib",
            steam_api_name: "libsteam_api.dylib",
        }
    } else {
        return Ok(());
    };

    println!("cargo:rerun-if-env-changed=STEAM_SDK_LOCATION");
    println!("cargo:rerun-if-changed=../steam-bridge/Cargo.toml");
    println!("cargo:rerun-if-changed=../steam-bridge/Cargo.lock");
    println!("cargo:rerun-if-changed=../steam-bridge/src/lib.rs");

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let workspace_dir = manifest_dir
        .parent()
        .ok_or_else(|| "failed to resolve workspace root".to_string())?;
    let bridge_manifest = workspace_dir.join("steam-bridge").join("Cargo.toml");
    let bridge_target_dir = manifest_dir.join("target").join("steam-bridge");
    let profile = env::var("PROFILE").map_err(|e| e.to_string())?;

    let status = Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(&bridge_manifest)
        .arg("--target")
        .arg(&target)
        .args(cargo_profile_args(&profile))
        .arg("--target-dir")
        .arg(&bridge_target_dir)
        .status()
        .map_err(|e| format!("failed to invoke cargo for steam bridge: {e}"))?;

    if !status.success() {
        return Err(format!("steam bridge build failed with status {}", status));
    }

    let app_profile_dir = find_profile_dir()?;
    let steam_dir = app_profile_dir.join("steam");
    fs::create_dir_all(&steam_dir).map_err(|e| {
        format!(
            "failed to create steam runtime directory {}: {e}",
            steam_dir.display()
        )
    })?;

    let bridge_output_dir = bridge_target_dir.join(&target).join(&profile);
    copy_if_newer(
        &bridge_output_dir.join(artifacts.bridge_source_name),
        &steam_dir.join(artifacts.bridge_staged_name),
    )?;

    let steam_api =
        find_file_named(&bridge_output_dir.join("build"), artifacts.steam_api_name).ok_or_else(
            || {
                format!(
                    "failed to locate {} in steam bridge build output; set STEAM_SDK_LOCATION to your Steamworks SDK root if needed",
                    artifacts.steam_api_name
                )
            },
        )?;
    copy_if_newer(&steam_api, &steam_dir.join(artifacts.steam_api_name))?;

    if target.contains("windows-gnu") {
        for dll_name in gnu_runtime_dlls(
            &target,
            &bridge_output_dir.join(artifacts.bridge_source_name),
        ) {
            if let Some(runtime_dll) = locate_linker_runtime_dll(&target, &dll_name) {
                copy_if_newer(&runtime_dll, &steam_dir.join(&dll_name))?;
            }
        }
    }

    Ok(())
}

struct SteamBridgeArtifacts {
    bridge_source_name: &'static str,
    bridge_staged_name: &'static str,
    steam_api_name: &'static str,
}

fn cargo_profile_args(profile: &str) -> Vec<&str> {
    match profile {
        "debug" => Vec::new(),
        "release" => vec!["--release"],
        _ => vec!["--profile", profile],
    }
}

fn find_profile_dir() -> Result<PathBuf, String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let profile = env::var("PROFILE").map_err(|e| e.to_string())?;

    for ancestor in out_dir.ancestors() {
        if ancestor.file_name().and_then(OsStr::to_str) == Some(profile.as_str()) {
            return Ok(ancestor.to_path_buf());
        }
    }

    Err(format!(
        "failed to resolve Cargo profile directory from OUT_DIR={}",
        out_dir.display()
    ))
}

fn copy_if_newer(src: &Path, dst: &Path) -> Result<(), String> {
    let src_meta = fs::metadata(src)
        .map_err(|e| format!("failed to read source file {}: {e}", src.display()))?;

    let should_copy = match fs::metadata(dst) {
        Ok(dst_meta) => {
            let src_mtime = src_meta.modified().ok();
            let dst_mtime = dst_meta.modified().ok();
            src_mtime.zip(dst_mtime).map(|(s, d)| s > d).unwrap_or(true)
        }
        Err(_) => true,
    };

    if should_copy {
        fs::copy(src, dst)
            .map_err(|e| format!("failed to copy {} -> {}: {e}", src.display(), dst.display()))?;
    }

    Ok(())
}

fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = fs::read_dir(&path).ok()?;
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry.file_name() == name {
                return Some(entry_path);
            }
            if entry.file_type().ok()?.is_dir() {
                stack.push(entry_path);
            }
        }
    }

    None
}

fn gnu_runtime_dlls(target: &str, bridge_dll: &Path) -> Vec<String> {
    let mut dlls = imported_dlls_from_objdump(target, bridge_dll).unwrap_or_else(|| {
        vec![
            "libgcc_s_seh-1.dll".to_string(),
            "libwinpthread-1.dll".to_string(),
            "libstdc++-6.dll".to_string(),
        ]
    });

    dlls.retain(|name| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".dll")
            && lower != "steam_api64.dll"
            && lower != "kernel32.dll"
            && lower != "user32.dll"
            && lower != "advapi32.dll"
            && lower != "shell32.dll"
            && lower != "ole32.dll"
            && lower != "ws2_32.dll"
            && lower != "ntdll.dll"
    });
    dlls.sort();
    dlls.dedup();
    dlls
}

fn imported_dlls_from_objdump(target: &str, bridge_dll: &Path) -> Option<Vec<String>> {
    let objdump = objdump_command_for_target(target)?;
    let output = Command::new(objdump)
        .arg("-p")
        .arg(bridge_dll)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let dlls = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("DLL Name: ").map(str::to_string))
        .collect::<Vec<_>>();

    if dlls.is_empty() {
        None
    } else {
        Some(dlls)
    }
}

fn locate_linker_runtime_dll(target: &str, dll_name: &str) -> Option<PathBuf> {
    let linker = linker_command_for_target(target)?;
    let output = Command::new(linker)
        .arg(format!("-print-file-name={dll_name}"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if path.is_empty() || path.eq_ignore_ascii_case(dll_name) {
        return None;
    }

    let resolved = PathBuf::from(path);
    resolved.exists().then_some(resolved)
}

fn linker_command_for_target(target: &str) -> Option<String> {
    let target_env = format!(
        "CARGO_TARGET_{}_LINKER",
        target.replace('-', "_").to_ascii_uppercase()
    );

    env::var(&target_env)
        .ok()
        .or_else(|| env::var("RUSTC_LINKER").ok())
        .or_else(|| {
            if target == "x86_64-pc-windows-gnu" {
                Some("x86_64-w64-mingw32-gcc".to_string())
            } else if target == "i686-pc-windows-gnu" {
                Some("i686-w64-mingw32-gcc".to_string())
            } else {
                None
            }
        })
}

fn objdump_command_for_target(target: &str) -> Option<String> {
    let linker = linker_command_for_target(target)?;
    if let Some(prefix) = linker.strip_suffix("-gcc") {
        return Some(format!("{prefix}-objdump"));
    }
    if let Some(prefix) = linker.strip_suffix("gcc.exe") {
        return Some(format!("{prefix}objdump.exe"));
    }
    Some("objdump".to_string())
}
