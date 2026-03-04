use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use super::{
    extractor::extract_zip,
    manifest::{Manifest, PackType},
};
use crate::server::PackEntry;

pub struct InstallResult {
    pub pack_name: String,
    pub pack_type: PackType,
}

/// Install all packs found in `source_path` into the server at `server_path`.
/// A single archive may contain both a BP and RP — both are installed.

pub fn install(
    source_path: &Path,
    server_path: &Path,
    custom_name: Option<String>,
) -> color_eyre::Result<Vec<InstallResult>> {
    let manifest_dir = if source_path.is_dir() {
        source_path.to_path_buf()
    } else {
        let tmp = server_path.join(".tmp_install");
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        extract_zip(source_path, &tmp)
            .map_err(|e| color_eyre::eyre::eyre!("Extraction failed: {e}"))?;
        tmp
    };

    let manifest_paths = find_all_manifests(&manifest_dir);
    if manifest_paths.is_empty() {
        cleanup_tmp(server_path);

        return Err(color_eyre::eyre::eyre!("No manifest.json found in archive"));
    }

    let mut results = Vec::new();
    let mut errors = Vec::new();

    for manifest_path in &manifest_paths {
        // If there's a custom name and it's the only pack, apply it to all manifests? Or first one?
        // We'll apply custom name to all manifests for simplicity, but user might expect it only for the first.
        // Better: if multiple manifests, we could ask? But for now, apply to all.
        let custom = custom_name.clone(); // clone for each
        match install_single(manifest_path, server_path, custom) {
            Ok(r) => results.push(r),
            Err(e) => errors.push(e.to_string()),
        }
    }

    cleanup_tmp(server_path);

    if results.is_empty() {
        return Err(color_eyre::eyre::eyre!("{}", errors.join("; ")));
    }

    Ok(results)
}

fn install_single(
    manifest_path: &Path,
    server_path: &Path,
    custom_name: Option<String>,
) -> color_eyre::Result<InstallResult> {
    let manifest_content = fs::read_to_string(manifest_path)?;
    let mut manifest: Manifest = serde_json::from_str(&manifest_content)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to parse manifest.json: {e}"))?;

    // Apply custom name if provided
    if let Some(name) = custom_name {
        manifest.header.name = Some(name);

        // Write back the modified manifest
        let modified_content = serde_json::to_string_pretty(&manifest)?;
        fs::write(manifest_path, modified_content)?;
    }

    let pack_type = manifest.pack_type();
    let pack_name = manifest
        .header
        .name
        .clone()
        .unwrap_or_else(|| manifest.header.uuid.clone());

    let (target_subdir, json_file) = match &pack_type {
        PackType::Resources => ("resource_packs", "resource_packs.json"),
        PackType::Behavior => ("behavior_packs", "behavior_packs.json"),

        PackType::Unknown => {
            return Err(color_eyre::eyre::eyre!(
                "Unknown pack type in {}",
                manifest_path.display()
            ));
        }
    };

    let pack_root = manifest_path.parent().expect("manifest has parent dir");

    let pack_dest = server_path.join(target_subdir).join(&manifest.header.uuid);

    copy_dir_all(pack_root, &pack_dest)?;

    let json_path = server_path.join(json_file);
    update_pack_list(&json_path, &manifest.header.uuid, &manifest.header.version)?;

    Ok(InstallResult {
        pack_name,
        pack_type,
    })
}

/// Enable or disable a pack by updating its JSON list file.
/// `should_enable = true` adds the pack; `false` removes it.
pub fn set_pack_enabled(
    json_path: &Path,
    uuid: &str,
    version: &[u32],
    should_enable: bool,
) -> color_eyre::Result<()> {
    let mut entries: Vec<PackEntry> = if json_path.exists() {
        let content = fs::read_to_string(json_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    if should_enable {
        if !entries.iter().any(|e| e.pack_id == uuid) {
            entries.push(PackEntry {
                pack_id: uuid.to_string(),
                version: version.to_vec(),
            });
        }
    } else {
        entries.retain(|e| e.pack_id != uuid);
    }

    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(json_path, serde_json::to_string_pretty(&entries)?)?;
    Ok(())
}

/// Recursively find all `manifest.json` files under `dir`.
fn find_all_manifests(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    collect_manifests(dir, &mut results);
    results
}

fn collect_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_manifests(&path, out);
        } else if path.file_name().is_some_and(|n| n == "manifest.json") {
            out.push(path);
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Read, deduplicate, and write `resource_packs.json` / `behavior_packs.json`.
fn update_pack_list(json_path: &Path, uuid: &str, version: &[u32]) -> color_eyre::Result<()> {
    if json_path.exists() {
        let mut bak_name: OsString = json_path
            .file_name()
            .expect("json_path has filename")
            .to_os_string();
        bak_name.push(".bak");
        let bak_path = json_path.with_file_name(bak_name);
        fs::copy(json_path, bak_path)?;
    }

    let mut entries: Vec<PackEntry> = if json_path.exists() {
        let content = fs::read_to_string(json_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    if !entries.iter().any(|e| e.pack_id == uuid) {
        entries.push(PackEntry {
            pack_id: uuid.to_string(),
            version: version.to_vec(),
        });
        if let Some(parent) = json_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(json_path, serde_json::to_string_pretty(&entries)?)?;
    }

    Ok(())
}

fn cleanup_tmp(server_path: &Path) {
    let tmp = server_path.join(".tmp_install");
    if tmp.exists() {
        let _ = fs::remove_dir_all(&tmp);
    }
}
