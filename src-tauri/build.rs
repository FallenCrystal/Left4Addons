use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=L4A_STAGE_STEAM_BRIDGE");
    tauri_build::build();

    if env::var("L4A_STAGE_STEAM_BRIDGE").as_deref() == Ok("1") {
        if let Err(err) = stage_steam_bridge() {
            println!("cargo:warning=failed to stage Steam bridge runtime files: {err}");
        }
    }
}

fn stage_steam_bridge() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|e| e.to_string())?;
    let (bridge_source_name, bridge_staged_name, steam_api_name) = if target.contains("windows") {
        ("l4a_steam_bridge.dll", "l4a-steam-bridge.dll", "steam_api64.dll")
    } else if target.contains("linux") {
        ("libl4a_steam_bridge.so", "libl4a-steam-bridge.so", "libsteam_api.so")
    } else if target.contains("darwin") {
        ("libl4a_steam_bridge.dylib", "libl4a-steam-bridge.dylib", "libsteam_api.dylib")
    } else {
        return Ok(());
    };

    println!("cargo:rerun-if-env-changed=STEAM_SDK_LOCATION");
    println!("cargo:rerun-if-env-changed=L4A_STAGE_STEAM_BRIDGE");
    println!("cargo:rerun-if-changed=../crates/l4a-steam-bridge/Cargo.toml");
    println!("cargo:rerun-if-changed=../crates/l4a-steam-bridge/src/lib.rs");

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let workspace_dir = manifest_dir
        .parent()
        .ok_or_else(|| "failed to resolve workspace root".to_string())?;
    let bridge_manifest = workspace_dir
        .join("crates")
        .join("l4a-steam-bridge")
        .join("Cargo.toml");
    let bridge_target_dir = manifest_dir.join("target").join("steam-bridge");
    let profile = env::var("PROFILE").map_err(|e| e.to_string())?;

    let profile_args: Vec<&str> = match profile.as_str() {
        "debug" => Vec::new(),
        "release" => vec!["--release"],
        other => vec!["--profile", other],
    };

    let status = Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(&bridge_manifest)
        .arg("--target")
        .arg(&target)
        .args(profile_args)
        .arg("--target-dir")
        .arg(&bridge_target_dir)
        .status()
        .map_err(|e| format!("failed to invoke cargo for steam bridge: {e}"))?;

    if !status.success() {
        return Err(format!("steam bridge build failed with status {status}"));
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let app_profile_dir = out_dir
        .ancestors()
        .find(|p| p.file_name().and_then(OsStr::to_str) == Some(profile.as_str()))
        .ok_or_else(|| "failed to resolve Cargo profile directory".to_string())?
        .to_path_buf();

    let steam_dir = app_profile_dir.join("steam");
    fs::create_dir_all(&steam_dir).map_err(|e| e.to_string())?;

    let bridge_output_dir = bridge_target_dir.join(&target).join(&profile);
    copy_if_newer(
        &bridge_output_dir.join(bridge_source_name),
        &steam_dir.join(bridge_staged_name),
    )?;

    if let Some(steam_api) = find_file_named(&bridge_output_dir.join("build"), steam_api_name) {
        copy_if_newer(&steam_api, &steam_dir.join(steam_api_name))?;
    }

    if target.contains("windows") && target.contains("gnu") {
        let gxx = format!("{target}-g++");
        for dll in [
            "libgcc_s_seh-1.dll",
            "libstdc++-6.dll",
            "libwinpthread-1.dll",
        ] {
            if let Ok(output) = Command::new(&gxx).arg(format!("-print-file-name={dll}")).output() {
                if output.status.success() {
                    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let src_path = Path::new(&resolved);
                    if src_path.is_file() {
                        let _ = copy_if_newer(src_path, &steam_dir.join(dll));
                    }
                }
            }
        }
    }

    Ok(())
}

fn copy_if_newer(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("missing artifact: {}", src.display()));
    }
    fs::copy(src, dst).map_err(|e| e.to_string())?;
    Ok(())
}

fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry.file_name() == name {
                return Some(entry_path);
            }
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                stack.push(entry_path);
            }
        }
    }
    None
}
