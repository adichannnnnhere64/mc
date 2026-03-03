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

/// Install a plugin/addon from `source_path` into the server at `server_path`.
pub fn install(source_path: &Path, server_path: &Path) -> color_eyre::Result<InstallResult> {
    // Resolve the directory containing the manifest
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

    let manifest_path = find_manifest(&manifest_dir)
        .ok_or_else(|| color_eyre::eyre::eyre!("manifest.json not found in archive"))?;

    let manifest_content = fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = serde_json::from_str(&manifest_content)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to parse manifest.json: {e}"))?;

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
            cleanup_tmp(server_path);
            return Err(color_eyre::eyre::eyre!(
                "Unknown pack type — check modules[].type in manifest.json"
            ));
        }
    };

    let pack_root = manifest_path.parent().expect("manifest has parent dir");
    let pack_dest = server_path
        .join(target_subdir)
        .join(&manifest.header.uuid);

    copy_dir_all(pack_root, &pack_dest)?;

    let json_path = server_path.join(json_file);
    update_pack_list(&json_path, &manifest.header.uuid, &manifest.header.version)?;

    cleanup_tmp(server_path);

    Ok(InstallResult {
        pack_name,
        pack_type,
    })
}

/// Recursively find the first `manifest.json` in `dir`.
fn find_manifest(dir: &Path) -> Option<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_manifest(&path) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == "manifest.json") {
            return Some(path);
        }
    }
    None
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
    // Backup existing file
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

    // Deduplicate by UUID
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
