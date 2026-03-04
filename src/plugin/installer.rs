use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::Value;

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
    let tmp = server_path.join(".tmp_install");
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }

    if source_path.is_dir() {
        // Copy the folder so nested archives can be expanded without modifying source.
        copy_dir_all(source_path, &tmp)
            .map_err(|e| color_eyre::eyre::eyre!("Copy failed: {e}"))?;
        super::extractor::expand_nested_archives(&tmp)
            .map_err(|e| color_eyre::eyre::eyre!("Extraction failed: {e}"))?;
    } else {
        extract_zip(source_path, &tmp)
            .map_err(|e| color_eyre::eyre::eyre!("Extraction failed: {e}"))?;
    }

    let manifest_dir = tmp;

    let manifest_paths = find_all_manifests(&manifest_dir);
    if manifest_paths.is_empty() {
        let sample = sample_extracted_files(&manifest_dir, 20);
        let detail = if sample.is_empty() {
            "archive extracted but produced no files".to_string()
        } else {
            format!("extracted files include: {}", sample.join(", "))
        };
        cleanup_tmp(server_path);

        return Err(color_eyre::eyre::eyre!(
            "No manifest.json found in archive ({detail})"
        ));
    }

    let mut results = Vec::new();
    let mut errors = Vec::new();

    for manifest_path in &manifest_paths {
        let custom = custom_name.clone();
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
    let mut manifest_value: Value = serde_json::from_str(&manifest_content)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to parse manifest.json: {e}"))?;
    let mut manifest: Manifest = serde_json::from_value(manifest_value.clone())
        .map_err(|e| color_eyre::eyre::eyre!("Failed to parse manifest.json: {e}"))?;

    // Apply custom name while preserving unknown manifest fields.
    if let Some(name) = custom_name {
        if let Some(header) = manifest_value.get_mut("header").and_then(Value::as_object_mut) {
            header.insert("name".to_string(), Value::String(name.clone()));
        }
        manifest.header.name = Some(name);
        let modified_content = serde_json::to_string_pretty(&manifest_value)?;
        fs::write(manifest_path, modified_content)?;
    }

    let pack_type = manifest.pack_type();
    let pack_name = manifest
        .header
        .name
        .clone()
        .unwrap_or_else(|| manifest.header.uuid.clone());

    let (pack_subdir, world_json_file) = match &pack_type {
        PackType::Resources => ("resource_packs", "world_resource_packs.json"),
        PackType::Behavior => ("behavior_packs", "world_behavior_packs.json"),
        PackType::Unknown => {
            return Err(color_eyre::eyre::eyre!(
                "Unknown pack type in {}",
                manifest_path.display()
            ));
        }
    };

    let pack_root = manifest_path.parent().ok_or_else(|| {
        color_eyre::eyre::eyre!("Invalid manifest path: {}", manifest_path.display())
    })?;

    // Install pack files and update JSON for the primary world and any others.
    let world_dirs = collect_world_dirs(server_path);
    for world_dir in &world_dirs {
        let pack_dest = world_dir.join(pack_subdir).join(&manifest.header.uuid);
        if pack_dest.exists() {
            fs::remove_dir_all(&pack_dest)?;
        }
        copy_dir_all(pack_root, &pack_dest)?;

        // Ensure destination has a canonical lowercase manifest filename.
        fs::write(
            pack_dest.join("manifest.json"),
            serde_json::to_string_pretty(&manifest_value)?,
        )?;

        validate_installed_manifest(
            &pack_dest.join("manifest.json"),
            &manifest.header.uuid,
            &manifest.header.version,
        )?;

        let json_path = world_dir.join(world_json_file);
        update_pack_list(&json_path, &manifest.header.uuid, &manifest.header.version)?;
    }

    Ok(InstallResult {
        pack_name,
        pack_type,
    })
}

/// Enable or disable a pack across all world directories.
pub fn set_pack_enabled(
    server_path: &Path,
    uuid: &str,
    version: &[u32],
    is_resource: bool,
    should_enable: bool,
) -> color_eyre::Result<()> {
    let world_json_file = if is_resource {
        "world_resource_packs.json"
    } else {
        "world_behavior_packs.json"
    };
    for world_dir in collect_world_dirs(server_path) {
        set_pack_enabled_in_file(&world_dir.join(world_json_file), uuid, version, should_enable)?;
    }
    Ok(())
}

fn set_pack_enabled_in_file(
    json_path: &Path,

    uuid: &str,
    version: &[u32],
    should_enable: bool,
) -> color_eyre::Result<()> {
    let mut entries: Vec<PackEntry> = if json_path.exists() {
        let content = fs::read_to_string(json_path)?;
        serde_json::from_str(&content)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to parse {}: {e}", json_path.display()))?
    } else {
        Vec::new()
    };

    let old = entries.clone();

    if should_enable {
        upsert_pack_entry(&mut entries, uuid, version);
    } else {
        entries.retain(|e| e.pack_id != uuid);
    }

    if entries != old {
        if let Some(parent) = json_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(json_path, serde_json::to_string_pretty(&entries)?)?;
    }
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
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("manifest.json"))
        {
            out.push(path);
        }
    }
}

fn sample_extracted_files(dir: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    collect_file_sample(dir, dir, limit, &mut out);
    out
}

fn collect_file_sample(root: &Path, dir: &Path, limit: usize, out: &mut Vec<String>) {
    if out.len() >= limit {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if out.len() >= limit {
            break;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_file_sample(root, &path, limit, out);
            continue;
        }

        let rel = path.strip_prefix(root).unwrap_or(&path);
        out.push(rel.display().to_string());
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

/// Read, deduplicate, and write pack list JSON files.
fn update_pack_list(json_path: &Path, uuid: &str, version: &[u32]) -> color_eyre::Result<()> {
    let mut entries: Vec<PackEntry> = if json_path.exists() {
        let content = fs::read_to_string(json_path)?;

        serde_json::from_str(&content)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to parse {}: {e}", json_path.display()))?
    } else {
        Vec::new()
    };

    let old = entries.clone();
    upsert_pack_entry(&mut entries, uuid, version);
    deduplicate_entries(&mut entries);

    if entries != old {
        // Backup existing file
        if json_path.exists() {
            let Some(file_name) = json_path.file_name() else {
                return Err(color_eyre::eyre::eyre!(
                    "Invalid pack list path (missing filename): {}",
                    json_path.display()
                ));
            };
            let mut bak_name: OsString = file_name.to_os_string();
            bak_name.push(".bak");
            let bak_path = json_path.with_file_name(bak_name);
            fs::copy(json_path, bak_path)?;
        }

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

/// Read `level-name` from `server.properties` (defaults to `"Bedrock level"`).
fn read_level_name(server_path: &Path) -> Option<String> {
    let content = fs::read_to_string(server_path.join("server.properties")).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("level-name=") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    Some("Bedrock level".to_string())
}

/// Return all world directories under `server_path/worlds/`, ensuring the
/// primary world (from `level-name` in server.properties) always exists.
fn collect_world_dirs(server_path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let worlds_root = detect_worlds_root(server_path);

    // Primary world — create it if it doesn't exist yet.
    let primary = worlds_root
        .join(read_level_name(server_path).unwrap_or_else(|| "Bedrock level".to_string()));
    let _ = fs::create_dir_all(&primary);
    dirs.push(primary.clone());

    // Any additional world directories that already exist.
    if let Ok(entries) = fs::read_dir(&worlds_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path != primary {
                dirs.push(path);
            }
        }
    }

    dirs
}

fn detect_worlds_root(server_path: &Path) -> PathBuf {
    let direct = server_path.join("worlds");
    if direct.exists() {
        return direct;
    }
    let data_worlds = server_path.join("data").join("worlds");
    if data_worlds.exists() {
        return data_worlds;
    }
    direct
}

fn validate_installed_manifest(
    manifest_path: &Path,
    expected_uuid: &str,
    expected_version: &[u32],
) -> color_eyre::Result<()> {
    let content = fs::read_to_string(manifest_path)?;
    let parsed: Manifest = serde_json::from_str(&content)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to parse {}: {e}", manifest_path.display()))?;
    if parsed.header.uuid != expected_uuid || parsed.header.version != expected_version {
        return Err(color_eyre::eyre::eyre!(
            "Installed manifest mismatch at {}",
            manifest_path.display()
        ));
    }
    Ok(())
}

fn upsert_pack_entry(entries: &mut Vec<PackEntry>, uuid: &str, version: &[u32]) {
    if let Some(entry) = entries.iter_mut().find(|e| e.pack_id == uuid) {
        entry.version = version.to_vec();
    } else {
        entries.push(PackEntry {
            pack_id: uuid.to_string(),
            version: version.to_vec(),
        });
    }
}

fn deduplicate_entries(entries: &mut Vec<PackEntry>) {
    let mut deduped: Vec<PackEntry> = Vec::with_capacity(entries.len());
    for entry in entries.drain(..) {
        if let Some(existing) = deduped.iter_mut().find(|e| e.pack_id == entry.pack_id) {
            existing.version = entry.version;
        } else {
            deduped.push(entry);
        }
    }
    *entries = deduped;
}
