use std::{
    fs,
    io::{self, Read, Seek},
    path::{Component, Path, PathBuf},
};

/// Extract a zip/mcaddon/mcpack archive into `target_dir`, then recursively
/// expand any nested archives found within.
pub fn extract_zip(archive_path: &Path, target_dir: &Path) -> io::Result<()> {
    let file = fs::File::open(archive_path)?;
    let archive = zip::ZipArchive::new(file)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    fs::create_dir_all(target_dir)?;
    extract_entries(archive, target_dir)?;
    expand_nested_archives(target_dir)
}

/// Write every entry in `archive` flat into `target_dir` (no nested expansion).
fn extract_entries<R: Read + Seek>(
    mut archive: zip::ZipArchive<R>,
    target_dir: &Path,
) -> io::Result<()> {
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let Some(relative_path) = normalize_entry_path(entry.name()) else {
            continue;
        };
        let out_path = target_dir.join(relative_path);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut out_file = fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out_file)?;
    }
    Ok(())
}

/// Scan `dir` for `.mcpack`, `.mcaddon`, and `.zip` files, extract each into
/// an adjacent subdirectory, then recurse into those subdirectories.
///
/// Used both by `extract_zip` (post-extraction pass) and by `installer` when
/// the install source is a folder that contains archives rather than a direct
/// `manifest.json`.
pub fn expand_nested_archives(dir: &Path) -> io::Result<()> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in rd.flatten() {
        let path = entry.path();

        if path.is_dir() {
            expand_nested_archives(&path)?;
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());

        if matches!(ext.as_deref(), Some("mcpack" | "mcaddon" | "zip")) {
            if let Ok(file) = fs::File::open(&path) {
                if let Ok(archive) = zip::ZipArchive::new(file) {
                    let target = unique_unpack_dir(dir, &path);
                    fs::create_dir_all(&target)?;
                    extract_entries(archive, &target)?;
                    expand_nested_archives(&target)?;
                }
            }
        }
    }

    Ok(())
}

fn unique_unpack_dir(target_dir: &Path, archive_path: &Path) -> PathBuf {
    let stem = archive_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("pack");

    let mut candidate = target_dir.join(stem);
    let mut n = 2usize;
    while candidate.exists() {
        candidate = target_dir.join(format!("{stem}_{n}"));
        n += 1;
    }
    candidate
}

fn normalize_entry_path(name: &str) -> Option<PathBuf> {
    // Some packs are authored on Windows and store '\' separators inside zips.
    let normalized = name.replace('\\', "/");
    if normalized.is_empty() {
        return None;
    }

    let path = Path::new(&normalized);
    let mut relative = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => return None,
        }
    }

    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}
