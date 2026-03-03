use std::{fs, io, path::Path};

/// Extract a zip or mcaddon archive into `target_dir`.
///
/// Basic path-traversal protection: entries containing `..` are skipped.
pub fn extract_zip(archive_path: &Path, target_dir: &Path) -> io::Result<()> {
    let file = fs::File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    fs::create_dir_all(target_dir)?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let entry_name = entry.name().to_string();

        // Prevent path traversal
        if entry_name.contains("..") {
            continue;
        }

        let out_path = target_dir.join(&entry_name);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = fs::File::create(&out_path)?;
            io::copy(&mut entry, &mut out_file)?;
        }
    }

    Ok(())
}
