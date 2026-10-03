//! Utility functions

/// Format download count for display
///
/// Converts a raw download count into a human-readable format:
/// - Millions: "1.5M", "10.2M"
/// - Thousands: "5.3K", "999.9K"
/// - Less than 1000: "123", "999"
pub fn format_download_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Write a cache file so readers never see a partial file.
/// Failures are ignored: caches are an optimization.
pub fn write_cache_file(path: &std::path::Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = std::path::PathBuf::from(temporary);
    if std::fs::write(&temporary, bytes).is_err() || std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::write_cache_file;

    #[test]
    fn cache_files_are_replaced_without_leaving_temporaries() {
        let dir = std::env::temp_dir().join(format!("kompas-cache-test-{}", std::process::id()));
        let path = dir.join("nested/value.json");
        write_cache_file(&path, b"one");
        write_cache_file(&path, b"two");
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
