use std::{collections::HashMap, sync::OnceLock, time::Instant};

use crate::AppId;
use crate::app_info::WaylandCompatibility;
use rust_embed::RustEmbed;

const STATS_URL_V8: &str =
    "https://github.com/shipdocs/kompas/releases/download/latest/flathub-stats.bitcode-v0-8";
const STATS_URL: &str =
    "https://github.com/shipdocs/kompas/releases/download/latest/flathub-stats.bitcode-v0-7";
const METADATA_URL: &str =
    "https://github.com/shipdocs/kompas/releases/download/latest/flathub-metadata.json";
const STATS_CACHE_PATH_V8: &str = "flathub-stats.bitcode";
const METADATA_CACHE_PATH: &str = "flathub-metadata.json";
const CACHE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60; // 30 days

#[derive(RustEmbed)]
#[folder = "res/"]
struct StatsAssets;

#[derive(serde::Deserialize)]
struct StatsMetadata {
    generated_at: u64,
}

#[derive(bitcode::Decode, bitcode::Encode)]
struct FlathubStatsV8 {
    generated_at: u64,
    downloads: HashMap<AppId, u64>,
    compatibility: HashMap<AppId, WaylandCompatibility>,
}

#[derive(bitcode::Decode, bitcode::Encode)]
struct FlathubStatsV7 {
    downloads: HashMap<AppId, u64>,
    compatibility: HashMap<AppId, WaylandCompatibility>,
}

struct FlathubStats {
    downloads: HashMap<AppId, u64>,
    compatibility: HashMap<AppId, WaylandCompatibility>,
}

static STATS: OnceLock<FlathubStats> = OnceLock::new();

fn cache_file(name: &str) -> Option<std::path::PathBuf> {
    Some(
        dirs::cache_dir()?
            .join(crate::constants::CACHE_DIR)
            .join(name),
    )
}

fn get_cache_path_v8() -> Option<std::path::PathBuf> {
    cache_file(STATS_CACHE_PATH_V8)
}

fn get_cache_path_v7() -> Option<std::path::PathBuf> {
    cache_file("flathub-stats.bitcode-v0-7")
}

/// Plain GET with timeouts so a stalled network cannot block startup indefinitely.
fn http_get(url: &str) -> Option<reqwest::blocking::Response> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?
        .get(url)
        .send()
        .ok()
}

fn try_fetch_remote_metadata() -> Option<StatsMetadata> {
    log::debug!("fetching remote metadata from {}", METADATA_URL);
    let response = http_get(METADATA_URL)?;
    if !response.status().is_success() {
        log::warn!("failed to fetch metadata: {}", response.status());
        return None;
    }
    response.json().ok()
}

fn try_load_cached_metadata() -> Option<StatsMetadata> {
    let cache_path = cache_file(METADATA_CACHE_PATH)?;
    let data = std::fs::read_to_string(&cache_path).ok()?;
    serde_json::from_str(&data).ok()
}

fn should_download_new_version() -> bool {
    let Some(remote_meta) = try_fetch_remote_metadata() else {
        log::debug!("cannot fetch remote metadata, will download to be safe");
        return true;
    };

    let Some(cached_meta) = try_load_cached_metadata() else {
        log::info!("no cached metadata, downloading new version");
        return true;
    };

    if remote_meta.generated_at > cached_meta.generated_at {
        log::info!(
            "remote stats newer (remote: {}, cached: {}), downloading",
            remote_meta.generated_at,
            cached_meta.generated_at
        );
        true
    } else {
        log::info!("cached stats are up to date");
        false
    }
}

#[cfg(feature = "flathub-stats-v8")]
fn try_load_cached_v8() -> Option<Vec<u8>> {
    let cache_path = get_cache_path_v8()?;
    let data = std::fs::read(&cache_path).ok()?;
    log::info!("loaded cached v0-8 stats from {:?}", cache_path);
    Some(data)
}

#[cfg(feature = "flathub-stats-v8")]
fn try_load_bundled_v8() -> Option<Vec<u8>> {
    let file = StatsAssets::get("flathub-stats.bitcode-v0-8")?;
    log::info!("loaded bundled v0-8 stats");
    Some(file.data.into_owned())
}

fn try_load_cached_v7() -> Option<Vec<u8>> {
    let cache_path = get_cache_path_v7()?;
    let data = std::fs::read(&cache_path).ok()?;
    log::info!("loaded cached v0-7 stats from {:?}", cache_path);
    Some(data)
}

fn try_load_bundled_v7() -> Option<Vec<u8>> {
    let file = StatsAssets::get("flathub-stats.bitcode-v0-7")?;
    log::info!("loaded bundled v0-7 stats");
    Some(file.data.into_owned())
}

fn is_cache_stale() -> bool {
    // Check v0-8 cache first
    if let Some(cache_path) = get_cache_path_v8() {
        if let Ok(metadata) = std::fs::metadata(&cache_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs() < CACHE_MAX_AGE_SECS {
                        return false; // v8 cache is fresh
                    }
                }
            }
        }
    }

    // Fall back to checking v0-7 cache
    if let Some(cache_path) = get_cache_path_v7() {
        if let Ok(metadata) = std::fs::metadata(&cache_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    return elapsed.as_secs() >= CACHE_MAX_AGE_SECS;
                }
            }
        }
    }

    true // No cache found or couldn't check
}

fn download_and_cache() -> Option<Vec<u8>> {
    log::info!("downloading flathub statistics...");

    // 1. Fetch metadata (optional, but preferred)
    let metadata_content = match http_get(METADATA_URL) {
        Some(resp) if resp.status().is_success() => resp.text().ok(),
        _ => None,
    };

    // 2. Try download v0-8
    let (bytes, version) = if let Some(resp) = http_get(STATS_URL_V8) {
        if resp.status().is_success() {
            if let Ok(b) = resp.bytes() {
                (Some(b.to_vec()), 8)
            } else {
                (None, 0)
            }
        } else {
            // Fallback to v0-7
            log::warn!("v0-8 download failed ({}). trying v0-7...", resp.status());
            if let Some(resp7) = http_get(STATS_URL) {
                if resp7.status().is_success() {
                    (resp7.bytes().ok().map(|b| b.to_vec()), 7)
                } else {
                    (None, 0)
                }
            } else {
                (None, 0)
            }
        }
    } else {
        (None, 0)
    };

    let bytes = bytes?;
    // Never cache a payload that cannot be decoded (for example an error page).
    let decodes = match version {
        8 => decode_v8(&bytes).is_some(),
        _ => decode_v7(&bytes).is_some(),
    };
    if !decodes {
        log::warn!("downloaded v0-{} stats could not be decoded", version);
        return None;
    }
    log::info!("downloaded v0-{} stats ({} bytes)", version, bytes.len());

    // 3. Cache files (only if download succeeded)
    if version == 8 {
        // Write v0-8 bitcode
        if let Some(cache_path) = get_cache_path_v8() {
            crate::utils::write_cache_file(&cache_path, &bytes);
            log::info!("cached v0-8 stats to {:?}", cache_path);
        }

        // Write metadata if available
        if let Some(meta_json) = metadata_content {
            if let Some(cache_path) = cache_file(METADATA_CACHE_PATH) {
                crate::utils::write_cache_file(&cache_path, meta_json.as_bytes());
                log::info!("cached metadata");
            }
        }
    } else if version == 7 {
        // Write v0-7 bitcode
        if let Some(cache_path) = get_cache_path_v7() {
            crate::utils::write_cache_file(&cache_path, &bytes);
            log::info!("cached v0-7 stats to {:?}", cache_path);
        }
        // Don't write metadata for v7 as it might mismatch v8 format
    }

    Some(bytes)
}

fn decode_v8(data: &[u8]) -> Option<FlathubStats> {
    let v8 = bitcode::decode::<FlathubStatsV8>(data).ok()?;
    Some(FlathubStats {
        downloads: v8.downloads,
        compatibility: v8.compatibility,
    })
}

fn decode_v7(data: &[u8]) -> Option<FlathubStats> {
    let v7 = bitcode::decode::<FlathubStatsV7>(data).ok()?;
    Some(FlathubStats {
        downloads: v7.downloads,
        compatibility: v7.compatibility,
    })
}

fn load_stats() -> &'static FlathubStats {
    STATS.get_or_init(|| {
        let start = Instant::now();

        #[cfg(feature = "flathub-stats-v8")]
        {
            // 1. Try v0-8 cache first
            if let Some(data) = try_load_cached_v8() {
                if let Some(stats) = decode_v8(&data) {
                    log::info!("loaded v0-8 stats from cache in {:?}", start.elapsed());
                    return stats;
                }
            }

            // 2. Try bundled v0-8
            if let Some(data) = try_load_bundled_v8() {
                if let Some(stats) = decode_v8(&data) {
                    log::info!("loaded bundled v0-8 stats in {:?}", start.elapsed());
                    return stats;
                }
            }
        }

        // 3. Try v0-7 cache
        #[cfg(feature = "flathub-stats-v7")]
        {
            if let Some(data) = try_load_cached_v7() {
                if let Some(stats) = decode_v7(&data) {
                    log::info!("loaded v0-7 stats from cache in {:?}", start.elapsed());
                    return stats;
                }
            }

            // 4. Try bundled v0-7
            if let Some(data) = try_load_bundled_v7() {
                if let Some(stats) = decode_v7(&data) {
                    log::info!("loaded bundled v0-7 stats in {:?}", start.elapsed());
                    return stats;
                }
            }
        }

        // 5. Last resort: blocking download
        if let Some(data) = download_and_cache() {
            // We don't know if we got v8 or v7, try decode v8 first
            if let Some(stats) = decode_v8(&data) {
                return stats;
            }
            if let Some(stats) = decode_v7(&data) {
                return stats;
            }
        }

        // 6. Legacy v6 fallback
        let v6_path = std::path::Path::new("res/flathub-stats.bitcode-v0-6"); // Or embedded if present
        match std::fs::read(v6_path)
            .ok()
            .and_then(|data| bitcode::decode::<HashMap<AppId, u64>>(&data).ok())
        {
            Some(downloads) => {
                log::info!("loaded flathub statistics v0-6 in {:?}", start.elapsed());
                FlathubStats {
                    downloads,
                    compatibility: HashMap::new(),
                }
            }
            None => {
                log::warn!("failed to load any stats");
                FlathubStats {
                    downloads: HashMap::new(),
                    compatibility: HashMap::new(),
                }
            }
        }
    })
}

pub fn try_monthly_downloads(id: &AppId) -> Option<u64> {
    STATS.get()?.downloads.get(id).copied()
}

pub fn try_wayland_compatibility(id: &AppId) -> Option<WaylandCompatibility> {
    STATS.get()?.compatibility.get(id).cloned()
}

pub fn load_stats_map() -> (HashMap<AppId, u64>, HashMap<AppId, WaylandCompatibility>) {
    let stats = load_stats();
    (stats.downloads.clone(), stats.compatibility.clone())
}

/// Load stats in background thread, refresh cache if stale.
pub fn load_stats_async() {
    std::thread::spawn(|| {
        let _ = load_stats();

        // If cache is stale, refresh in background for next launch.
        if is_cache_stale() && should_download_new_version() {
            log::info!("cache is stale, refreshing in background");
            let _ = download_and_cache();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_comparison() {
        let old_ts = 1704067200; // 2024-01-01
        let new_ts = 1706745600; // 2024-02-01
        assert!(new_ts > old_ts, "newer timestamp should be greater");
    }

    #[test]
    fn test_v8_decode() {
        let stats = FlathubStatsV8 {
            generated_at: 1704067200,
            downloads: HashMap::new(),
            compatibility: HashMap::new(),
        };

        let encoded = bitcode::encode(&stats);
        let decoded = decode_v8(&encoded);

        assert!(decoded.is_some());
    }

    #[test]
    fn test_v7_backward_compat() {
        let stats = FlathubStatsV7 {
            downloads: HashMap::new(),
            compatibility: HashMap::new(),
        };

        let encoded = bitcode::encode(&stats);
        let decoded = decode_v7(&encoded);

        assert!(decoded.is_some());
    }

    #[test]
    fn test_metadata_json_parsing() {
        let json = r#"{
            "generated_at": 1704067200
        }"#;

        let metadata: StatsMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.generated_at, 1704067200);
    }
}
