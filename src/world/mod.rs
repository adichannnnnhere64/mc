use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::plugin::extractor::extract_zip;

#[derive(Debug, Clone, Copy)]
pub enum WorldImportMode {
    Create,
    Modify,
}

impl WorldImportMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Modify => "modify",
        }
    }
}

pub fn import_mcworld(
    server_path: &Path,
    archive_path: &Path,
    mode: WorldImportMode,
    target_world: Option<String>,
) -> color_eyre::Result<String> {
    if !archive_path.exists() || !archive_path.is_file() {
        return Err(color_eyre::eyre::eyre!(
            "mcworld file not found: {}",
            archive_path.display()
        ));
    }

    if !has_extension(archive_path, "mcworld") {
        return Err(color_eyre::eyre::eyre!(
            "Unsupported world archive: {} (expected .mcworld)",
            archive_path.display()
        ));
    }

    let worlds_root = detect_worlds_root(server_path);
    fs::create_dir_all(&worlds_root)?;

    let tmp_dir = server_path.join(".tmp_world_import");
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    extract_zip(archive_path, &tmp_dir)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to extract mcworld: {e}"))?;

    let source_world = find_world_dir(&tmp_dir).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "No world data found in archive (missing level.dat after extraction)"
        )
    })?;

    let result = match mode {
        WorldImportMode::Create => {
            let world_name = target_world
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| derive_default_world_name(archive_path));

            let dest = worlds_root.join(&world_name);
            if dest.exists() {
                cleanup_tmp(&tmp_dir);
                return Err(color_eyre::eyre::eyre!(
                    "World '{}' already exists. Use modify mode or a different name.",
                    world_name
                ));
            }

            copy_dir_all(&source_world, &dest)?;
            format!("World created: '{world_name}'")
        }
        WorldImportMode::Modify => {
            let world_name = target_world
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| color_eyre::eyre::eyre!("Target world name is required"))?
                .to_string();

            let dest = worlds_root.join(&world_name);
            if !dest.exists() {
                let available = list_world_dirs(&worlds_root);
                cleanup_tmp(&tmp_dir);
                return Err(color_eyre::eyre::eyre!(
                    "World '{}' not found. Available: {}",
                    world_name,
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                ));
            }

            replace_world_dir(&source_world, &dest)?;
            format!("World updated: '{world_name}'")
        }
    };

    cleanup_tmp(&tmp_dir);
    Ok(result)
}

fn replace_world_dir(source: &Path, destination: &Path) -> color_eyre::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        color_eyre::eyre::eyre!("Invalid world destination: {}", destination.display())
    })?;
    let world_name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid world destination name"))?;
    let backup = unique_backup_path(parent, world_name);

    fs::rename(destination, &backup)?;

    let restore_backup = |dest: &Path, bak: &Path| -> color_eyre::Result<()> {
        if dest.exists() {
            fs::remove_dir_all(dest)?;
        }
        fs::rename(bak, dest)?;
        Ok(())
    };

    match copy_dir_all(source, destination) {
        Ok(()) => {
            fs::remove_dir_all(&backup)?;
            Ok(())
        }
        Err(e) => {
            let _ = restore_backup(destination, &backup);
            Err(color_eyre::eyre::eyre!("Failed to update world: {e}"))
        }
    }
}

fn unique_backup_path(parent: &Path, name: &str) -> PathBuf {
    let mut index = 1usize;
    loop {
        let candidate = parent.join(format!(".{name}.import_backup_{index}"));
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

fn derive_default_world_name(archive_path: &Path) -> String {
    archive_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("ImportedWorld")
        .to_string()
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
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

fn find_world_dir(root: &Path) -> Option<PathBuf> {
    if root.join("level.dat").exists() {
        return Some(root.to_path_buf());
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && let Some(found) = find_world_dir(&path)
        {
            return Some(found);
        }
    }
    None
}

fn list_world_dirs(worlds_root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(worlds_root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    names.sort_unstable();
    names
}

fn cleanup_tmp(tmp_dir: &Path) {
    if tmp_dir.exists() {
        let _ = fs::remove_dir_all(tmp_dir);
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    if !src.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("source does not exist: {}", src.display()),
        ));
    }

    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
