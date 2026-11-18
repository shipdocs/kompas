use crate::app_entry::{AppEntry, Apps};
use crate::app_info::{AppKind, AppProvide, RiskLevel};
use crate::backend::Backends;
use crate::category::Category;
use crate::editors_choice::EDITORS_CHOICE;
use crate::pages::ExplorePage;
// Re-export and use Search types
use crate::app_info::WaylandCompatibility;
use crate::localize::LANGUAGE_SORTER;
pub use crate::search::{SearchResult, SearchSortMode, WaylandFilter};
use rayon::prelude::*;
use std::cmp;
use std::path::Path;
use std::time::Instant;

/// Pure function moved from App::generic_search
/// Pure function moved from App::generic_search
pub fn generic_search<
    F: Fn(
            &crate::app_id::AppId,
            &crate::app_info::AppInfo,
            bool,
            Option<u64>,
            Option<WaylandCompatibility>,
        ) -> Option<i64>
        + Send
        + Sync,
>(
    apps: &Apps,
    _backends: &Backends,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
    _os_codename: &str,
    filter_map: F,
    sort_mode: SearchSortMode,
    wayland_filter: WaylandFilter,
) -> Vec<SearchResult> {
    let search_start = Instant::now();

    let mut results: Vec<SearchResult> = apps
        .par_iter()
        .filter_map(|(id, infos)| {
            let stats = app_stats.get(id);
            let stats_downloads = stats.map(|(downloads, _)| *downloads);
            let stats_compat = stats.and_then(|(_, compatibility)| *compatibility);

            let mut best_weight: Option<i64> = None;
            for AppEntry {
                backend_name,
                info,
                installed,
            } in infos.iter()
            {
                if !entry_available(backend_name, info, *installed, _backends) {
                    continue;
                }

                if let Some(weight) =
                    filter_map(id, info, *installed, stats_downloads, stats_compat)
                {
                    if let Some(prev_weight) = best_weight
                        && prev_weight <= weight
                    {
                        continue;
                    }

                    best_weight = Some(weight);
                }
            }
            let weight = best_weight?;
            // Use first info as it is preferred, even if other ones had a higher weight
            let AppEntry {
                backend_name,
                info,
                installed: _,
            } = infos.iter().find(|entry| {
                entry_available(entry.backend_name, &entry.info, entry.installed, _backends)
            })?;

            if wayland_filter != WaylandFilter::All {
                let compat_opt = stats_compat.or_else(|| info.wayland_compat_lazy());
                let matches_filter = match wayland_filter {
                    WaylandFilter::All => true,
                    WaylandFilter::Excellent => compat_opt
                        .map(|c| c.risk_level == RiskLevel::Low)
                        .unwrap_or(false),
                    WaylandFilter::Good => compat_opt
                        .map(|c| c.risk_level == RiskLevel::Medium)
                        .unwrap_or(false),
                    WaylandFilter::Caution => compat_opt
                        .map(|c| c.risk_level == RiskLevel::High)
                        .unwrap_or(false),
                    WaylandFilter::Limited => compat_opt
                        .map(|c| c.risk_level == RiskLevel::Critical)
                        .unwrap_or(false),
                    WaylandFilter::Unknown => compat_opt.is_none(),
                };

                if !matches_filter {
                    return None;
                }
            }

            Some(SearchResult::new(
                backend_name,
                id.clone(),
                None,
                info.clone(),
                weight,
            ))
        })
        .collect();

    sort_results(&mut results, sort_mode, app_stats);

    // Icons are now loaded lazily in the view layer to avoid expensive I/O during search
    log::warn!("Search algorithm took {:?}", search_start.elapsed());
    results
}

pub fn sort_results(
    results: &mut [SearchResult],
    sort_mode: SearchSortMode,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
) {
    match sort_mode {
        SearchSortMode::Name => {
            results.par_sort_unstable_by(|a, b| LANGUAGE_SORTER.compare(&a.info.name, &b.info.name))
        }
        SearchSortMode::Relevance => {
            results.par_sort_unstable_by(|a, b| match a.weight.cmp(&b.weight) {
                cmp::Ordering::Equal => match LANGUAGE_SORTER.compare(&a.info.name, &b.info.name) {
                    cmp::Ordering::Equal => {
                        LANGUAGE_SORTER.compare(a.backend_name(), b.backend_name())
                    }
                    ordering => ordering,
                },
                ordering => ordering,
            });
        }
        SearchSortMode::MostDownloads => {
            results.par_sort_unstable_by(|a, b| {
                let a_downloads = app_stats
                    .get(&a.id)
                    .map(|(d, _)| *d)
                    .unwrap_or(a.info.monthly_downloads);
                let b_downloads = app_stats
                    .get(&b.id)
                    .map(|(d, _)| *d)
                    .unwrap_or(b.info.monthly_downloads);
                match b_downloads.cmp(&a_downloads) {
                    cmp::Ordering::Equal => LANGUAGE_SORTER.compare(&a.info.name, &b.info.name),
                    ordering => ordering,
                }
            });
        }
        SearchSortMode::NewestGames => {
            results.par_sort_unstable_by(|a, b| {
                b.info
                    .first_release
                    .cmp(&a.info.first_release)
                    .then_with(|| LANGUAGE_SORTER.compare(&a.info.name, &b.info.name))
            });
        }
        SearchSortMode::RecentlyUpdated => {
            results.par_sort_unstable_by(|a, b| {
                let a_timestamp = a.info.releases.first().and_then(|r| r.timestamp);
                let b_timestamp = b.info.releases.first().and_then(|r| r.timestamp);
                match b_timestamp.cmp(&a_timestamp) {
                    cmp::Ordering::Equal => LANGUAGE_SORTER.compare(&a.info.name, &b.info.name),
                    ordering => ordering,
                }
            });
        }
        SearchSortMode::BestWaylandSupport => {
            // Pre-compute all wayland risk levels to avoid repeated file I/O during sorting
            use std::collections::HashMap;
            let risk_cache: HashMap<crate::app_id::AppId, _> = results
                .par_iter()
                .map(|result| {
                    let risk = app_stats
                        .get(&result.id)
                        .and_then(|(_, c)| c.as_ref())
                        .map(|c| c.risk_level)
                        .or_else(|| result.info.wayland_compat_lazy().map(|c| c.risk_level))
                        .unwrap_or(RiskLevel::Critical);
                    (result.id.clone(), risk)
                })
                .collect();

            results.par_sort_unstable_by(|a, b| {
                let a_risk = risk_cache
                    .get(&a.id)
                    .copied()
                    .unwrap_or(RiskLevel::Critical);
                let b_risk = risk_cache
                    .get(&b.id)
                    .copied()
                    .unwrap_or(RiskLevel::Critical);

                // Lower risk level = better (Low=0, Medium=1, High=2, Critical=3)
                let a_score = match a_risk {
                    RiskLevel::Low => 0,
                    RiskLevel::Medium => 1,
                    RiskLevel::High => 2,
                    RiskLevel::Critical => 3,
                };
                let b_score = match b_risk {
                    RiskLevel::Low => 0,
                    RiskLevel::Medium => 1,
                    RiskLevel::High => 2,
                    RiskLevel::Critical => 3,
                };

                match a_score.cmp(&b_score) {
                    cmp::Ordering::Equal => LANGUAGE_SORTER.compare(&a.info.name, &b.info.name),
                    ordering => ordering,
                }
            });
        }
    }
}

/// Extracted search logic
pub fn search_results(
    apps: &Apps,
    backends: &Backends,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
    os_codename: &str,
    input: &str,
    sort_mode: SearchSortMode,
    wayland_filter: WaylandFilter,
) -> Vec<SearchResult> {
    let input = input.trim();
    if input.starts_with("/") && Path::new(&input).is_file() {
        return Vec::new(); // File paths handled by url_handlers in main
    }
    // GStreamer codec handled by url_handlers in main

    let pattern = regex::escape(input);
    let regex = match regex::RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
    {
        Ok(ok) => ok,
        Err(err) => {
            log::warn!("failed to parse regex {:?}: {}", pattern, err);
            return Vec::new();
        }
    };

    generic_search(
        apps,
        backends,
        app_stats,
        os_codename,
        |id,
         info,
         _installed,
         stats_downloads: Option<u64>,
         _stats_compat: Option<WaylandCompatibility>| {
            if !matches!(info.kind, AppKind::DesktopApplication) {
                return None;
            }
            //TODO: improve performance
            let stats_weight = |weight: i64| -> i64 {
                //TODO: make sure no overflows
                let downloads = stats_downloads.unwrap_or(info.monthly_downloads);
                (weight << 56) - (downloads as i64)
            };

            let alias_matches = |alias: &str| {
                alias
                    .trim_end_matches(".desktop")
                    .rsplit('.')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case(input))
            };
            if alias_matches(id.normalized()) || info.desktop_ids.iter().any(|id| alias_matches(id))
            {
                return Some(stats_weight(0));
            }

            //TODO: fuzzy match (nucleus-matcher?)
            let regex_weight = |string: &str, weight: i64| -> Option<i64> {
                let mat = regex.find(string)?;
                if mat.range().start == 0 {
                    if mat.range().end == string.len() {
                        Some(stats_weight(weight))
                    } else {
                        Some(stats_weight(weight + 1))
                    }
                } else {
                    Some(stats_weight(weight + 2))
                }
            };
            if let Some(weight) = regex_weight(&info.name, 0) {
                return Some(weight);
            }
            if let Some(weight) = regex_weight(&info.summary, 3) {
                return Some(weight);
            }
            if let Some(weight) = regex_weight(&info.description, 6) {
                return Some(weight);
            }
            if crate::catalog::alternatives(input)
                .iter()
                .any(|name| info.name.eq_ignore_ascii_case(name))
            {
                return Some(stats_weight(9));
            }
            None
        },
        sort_mode,
        wayland_filter,
    )
}

/// Extracted categories logic
pub fn categories_results(
    apps: &Apps,
    backends: &Backends,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
    os_codename: &str,
    categories: &[Category],
) -> Vec<SearchResult> {
    let applet_provide = AppProvide::Id("com.system76.CosmicApplet".to_string());
    generic_search(
        apps,
        backends,
        app_stats,
        os_codename,
        |_id,
         info,
         _installed,
         stats_downloads: Option<u64>,
         _stats_compat: Option<WaylandCompatibility>| {
            if !matches!(info.kind, AppKind::DesktopApplication) {
                return None;
            }
            let downloads = stats_downloads.unwrap_or(info.monthly_downloads);
            if categories.is_empty() {
                return Some(-(downloads as i64));
            }
            for category in categories {
                //TODO: this hack makes it easier to add applets to the nav bar
                if matches!(category, Category::CosmicApplet) {
                    if info.provides.contains(&applet_provide) {
                        return Some(-(downloads as i64));
                    }
                } else {
                    //TODO: contains doesn't work due to type mismatch
                    if info.categories.iter().any(|x| x == category.id()) {
                        return Some(-(downloads as i64));
                    }
                }
            }
            None
        },
        SearchSortMode::Relevance,
        WaylandFilter::All,
    )
}

/// Extracted explore page logic
#[allow(dead_code)]
pub fn explore_results_data(
    apps: &Apps,
    backends: &Backends,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
    os_codename: &str,
    explore_page: ExplorePage,
    now: i64,
) -> Vec<SearchResult> {
    let mut results = match explore_page {
        ExplorePage::EditorsChoice => generic_search(
            apps,
            backends,
            app_stats,
            os_codename,
            |id,
             _info,
             _installed,
             _stats_downloads: Option<u64>,
             _stats_compat: Option<WaylandCompatibility>| {
                EDITORS_CHOICE
                    .iter()
                    .position(|choice_id| choice_id == &id.normalized())
                    .map(|x| x as i64)
            },
            SearchSortMode::Relevance,
            WaylandFilter::All,
        ),
        ExplorePage::PopularApps => generic_search(
            apps,
            backends,
            app_stats,
            os_codename,
            |_id,
             info,
             _installed,
             stats_downloads: Option<u64>,
             _stats_compat: Option<WaylandCompatibility>| {
                if !matches!(info.kind, AppKind::DesktopApplication) {
                    return None;
                }
                let downloads = stats_downloads.unwrap_or(info.monthly_downloads);
                Some(-(downloads as i64))
            },
            SearchSortMode::Relevance,
            WaylandFilter::All,
        ),
        ExplorePage::MadeForCosmic => {
            let provide = AppProvide::Id("com.system76.CosmicApplication".to_string());
            generic_search(
                apps,
                backends,
                app_stats,
                os_codename,
                |_id,
                 info,
                 _installed,
                 stats_downloads: Option<u64>,
                 _stats_compat: Option<WaylandCompatibility>| {
                    if !matches!(info.kind, AppKind::DesktopApplication) {
                        return None;
                    }
                    if info.provides.contains(&provide) {
                        let downloads = stats_downloads.unwrap_or(info.monthly_downloads);
                        Some(-(downloads as i64))
                    } else {
                        None
                    }
                },
                SearchSortMode::Relevance,
                WaylandFilter::All,
            )
        }
        ExplorePage::LinuxGames => generic_search(
            apps,
            backends,
            app_stats,
            os_codename,
            |_id, info, _installed, _downloads, _compat| {
                (info
                    .categories
                    .iter()
                    .any(|c| c == crate::catalog::NATIVE_LINUX)
                    && info.categories.iter().any(|c| c == "Game"))
                .then_some(0)
            },
            SearchSortMode::Relevance,
            WaylandFilter::All,
        ),
        ExplorePage::NewApps => generic_search(
            apps,
            backends,
            app_stats,
            os_codename,
            |_id,
             info,
             _installed,
             _stats_downloads: Option<u64>,
             _stats_compat: Option<WaylandCompatibility>| {
                info.categories
                    .iter()
                    .any(|c| c == crate::catalog::NEW_RELEASE)
                    .then_some(0)
            },
            SearchSortMode::Relevance,
            WaylandFilter::All,
        ),
        ExplorePage::RecentlyUpdated => generic_search(
            apps,
            backends,
            app_stats,
            os_codename,
            |id,
             info,
             _installed,
             _stats_downloads: Option<u64>,
             _stats_compat: Option<WaylandCompatibility>| {
                if !matches!(info.kind, AppKind::DesktopApplication) {
                    return None;
                }
                // Finds the newest release and sorts from newest to oldest
                //TODO: appstream release info is often incomplete
                let mut min_weight = 0;
                for release in info.releases.iter() {
                    if let Some(timestamp) = release.timestamp {
                        if timestamp < now {
                            let weight = -timestamp;
                            if weight < min_weight {
                                min_weight = weight;
                            }
                        } else {
                            log::info!(
                                "{:?} has release timestamp {} which is past the present {}",
                                id,
                                timestamp,
                                now
                            );
                        }
                    }
                }
                Some(min_weight)
            },
            SearchSortMode::Relevance,
            WaylandFilter::All,
        ),
        _ => {
            let categories = explore_page.categories();
            generic_search(
                apps,
                backends,
                app_stats,
                os_codename,
                |_id,
                 info,
                 _installed,
                 stats_downloads: Option<u64>,
                 _stats_compat: Option<WaylandCompatibility>| {
                    if !matches!(info.kind, AppKind::DesktopApplication) {
                        return None;
                    }
                    let downloads = stats_downloads.unwrap_or(info.monthly_downloads);
                    for category in categories {
                        //TODO: contains doesn't work due to type mismatch
                        if info.categories.iter().any(|x| x == category.id()) {
                            return Some(-(downloads as i64));
                        }
                    }
                    None
                },
                SearchSortMode::Relevance,
                WaylandFilter::All,
            )
        }
    };
    sort_explore_results(&mut results, explore_page, app_stats);
    results
}

/// Discovery game rows follow original release dates, including after an async refresh.
fn sort_explore_results(
    results: &mut [SearchResult],
    page: ExplorePage,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
) {
    let mode = if matches!(page, ExplorePage::Games | ExplorePage::LinuxGames) {
        SearchSortMode::NewestGames
    } else {
        SearchSortMode::Relevance
    };
    sort_results(results, mode, app_stats);
}

/// Extracted installed apps logic
pub fn installed_results_data(
    apps: &Apps,
    backends: &Backends,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
    os_codename: &str,
) -> Vec<SearchResult> {
    generic_search(
        apps,
        backends,
        app_stats,
        os_codename,
        |id,
         _info,
         installed,
         _stats_downloads: Option<u64>,
         _stats_compat: Option<WaylandCompatibility>| {
            if installed {
                Some(if id.is_system() { -1 } else { 0 })
            } else {
                None
            }
        },
        SearchSortMode::Relevance,
        WaylandFilter::All,
    )
}

/// Calculate weight for a single explore page result
fn calculate_explore_weight(
    id: &crate::app_id::AppId,
    info: &crate::app_info::AppInfo,
    explore_page: ExplorePage,
    downloads: u64,
    now: i64,
) -> Option<i64> {
    match explore_page {
        ExplorePage::EditorsChoice => EDITORS_CHOICE
            .iter()
            .position(|choice_id| choice_id == &id.normalized())
            .map(|x| x as i64),
        ExplorePage::PopularApps => {
            if !matches!(info.kind, AppKind::DesktopApplication) {
                return None;
            }
            Some(-(downloads as i64))
        }
        ExplorePage::MadeForCosmic => {
            if !matches!(info.kind, AppKind::DesktopApplication) {
                return None;
            }
            let provide = AppProvide::Id("com.system76.CosmicApplication".to_string());
            if info.provides.contains(&provide) {
                Some(-(downloads as i64))
            } else {
                None
            }
        }
        ExplorePage::LinuxGames => (info
            .categories
            .iter()
            .any(|c| c == crate::catalog::NATIVE_LINUX)
            && info.categories.iter().any(|c| c == "Game"))
        .then_some(0),
        ExplorePage::NewApps => info
            .categories
            .iter()
            .any(|c| c == crate::catalog::NEW_RELEASE)
            .then_some(0),
        ExplorePage::RecentlyUpdated => {
            if !matches!(info.kind, AppKind::DesktopApplication) {
                return None;
            }
            let mut min_weight = 0;
            for release in info.releases.iter() {
                if let Some(timestamp) = release.timestamp
                    && timestamp < now
                {
                    let weight = -timestamp;
                    if weight < min_weight {
                        min_weight = weight;
                    }
                }
            }
            Some(min_weight)
        }
        _ => {
            // Category-based explore pages
            if !matches!(info.kind, AppKind::DesktopApplication) {
                return None;
            }
            let categories = explore_page.categories();
            if categories.is_empty() {
                return None;
            }
            if info
                .categories
                .iter()
                .any(|x| categories.iter().any(|c| x == c.id()))
            {
                Some(-(downloads as i64))
            } else {
                None
            }
        }
    }
}

/// Batch all explore page searches into a single pass over all apps
/// This is O(N) instead of O(13N) for running each search separately
pub fn explore_results_all(
    apps: &Apps,
    _backends: &Backends,
    app_stats: &std::collections::HashMap<
        crate::app_id::AppId,
        (u64, Option<WaylandCompatibility>),
    >,
    _os_codename: &str,
    now: i64,
) -> std::collections::HashMap<ExplorePage, Vec<SearchResult>> {
    use std::collections::HashMap;

    let mut results_map: HashMap<ExplorePage, Vec<SearchResult>> = HashMap::new();

    // Initialize empty result vectors for all explore pages
    for page in ExplorePage::all().iter() {
        results_map.insert(*page, Vec::new());
    }

    // Single pass over all apps
    for (id, infos) in apps.iter() {
        let stats_downloads = app_stats.get(id).map(|(downloads, _)| *downloads);

        // Use first info as it is preferred
        let Some(AppEntry {
            backend_name,
            info,
            installed,
        }) = infos.iter().find(|entry| {
            entry_available(entry.backend_name, &entry.info, entry.installed, _backends)
        })
        else {
            continue;
        };

        if !entry_available(backend_name, info, *installed, _backends) {
            continue;
        }

        let downloads = stats_downloads.unwrap_or(info.monthly_downloads);

        // Check all explore pages for this app
        for explore_page in ExplorePage::all().iter() {
            // Calculate weight for this explore page
            if let Some(weight) = calculate_explore_weight(id, info, *explore_page, downloads, now)
            {
                let result =
                    SearchResult::new(backend_name, id.clone(), None, info.clone(), weight);
                results_map.get_mut(explore_page).unwrap().push(result);
            }
        }
    }

    // Apply the same ordering to batch loads and individual page refreshes.
    for (page, results) in &mut results_map {
        sort_explore_results(results, *page, app_stats);
    }

    results_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_id::AppId;
    use crate::app_info::AppInfo;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn sample_apps() -> Apps {
        let mut apps = Apps::new();
        for (id, name, downloads) in [
            ("org.example.Alpha", "Alpha", 10),
            ("org.example.Zulu", "Zulu", 100),
        ] {
            let info = AppInfo {
                name: name.to_string(),
                kind: AppKind::DesktopApplication,
                monthly_downloads: downloads,
                ..AppInfo::default()
            };
            apps.insert(
                AppId::new(id),
                vec![AppEntry {
                    backend_name: "flatpak-system",
                    info: Arc::new(info),
                    installed: false,
                }],
            );
        }
        apps
    }

    #[derive(Debug)]
    struct UnavailableSystemBackend;

    impl crate::backend::Backend for UnavailableSystemBackend {
        fn load_caches(&mut self, _: bool) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
        fn info_caches(&self) -> &[crate::AppstreamCache] {
            &[]
        }
        fn installed(&self) -> Result<Vec<crate::backend::Package>, Box<dyn std::error::Error>> {
            Ok(Vec::new())
        }
        fn updates(&self) -> Result<Vec<crate::backend::Package>, Box<dyn std::error::Error>> {
            Ok(Vec::new())
        }
        fn file_packages(
            &self,
            _: &str,
        ) -> Result<Vec<crate::backend::Package>, Box<dyn std::error::Error>> {
            Ok(Vec::new())
        }
        fn operation(
            &self,
            _: &crate::Operation,
            _: Box<dyn FnMut(f32) + 'static>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
        fn is_package_available(&self, _: &[String]) -> bool {
            false
        }
    }

    #[test]
    fn newest_games_sorts_launch_dates_and_does_not_promote_recent_updates() {
        let mut results = Vec::new();
        for (name, first_release) in [("Old", Some(100)), ("Unknown", None), ("New", Some(200))] {
            results.push(SearchResult::new(
                "steam",
                AppId::new(name),
                None,
                Arc::new(AppInfo {
                    name: name.to_string(),
                    first_release,
                    releases: vec![crate::app_info::AppRelease {
                        timestamp: Some(9999),
                        version: "1".into(),
                        description: None,
                        url: None,
                    }],
                    ..AppInfo::default()
                }),
                0,
            ));
        }
        sort_results(&mut results, SearchSortMode::NewestGames, &HashMap::new());
        assert_eq!(
            results
                .iter()
                .map(|r| r.info.name.as_str())
                .collect::<Vec<_>>(),
            vec!["New", "Old", "Unknown"]
        );
    }

    #[test]
    fn discovery_game_rows_put_unknown_dates_last_in_batch_and_refresh() {
        let mut apps = Apps::new();
        for (name, date, downloads) in [
            ("Unknown", None, 1_000_000),
            ("Old", Some(100), 100),
            ("New", Some(200), 1),
        ] {
            apps.insert(
                AppId::new(name),
                vec![AppEntry {
                    backend_name: "steam",
                    info: Arc::new(AppInfo {
                        name: name.into(),
                        kind: AppKind::DesktopApplication,
                        categories: vec!["Game".into(), crate::catalog::NATIVE_LINUX.into()],
                        first_release: date,
                        monthly_downloads: downloads,
                        ..AppInfo::default()
                    }),
                    installed: false,
                }],
            );
        }
        let backends = Backends::new();
        let stats = HashMap::new();
        let batch = explore_results_all(&apps, &backends, &stats, "noble", 300);
        for page in [ExplorePage::Games, ExplorePage::LinuxGames] {
            let refreshed = explore_results_data(&apps, &backends, &stats, "noble", page, 300);
            for results in [&batch[&page], &refreshed] {
                assert_eq!(
                    results
                        .iter()
                        .map(|r| r.info.name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["New", "Old", "Unknown"]
                );
            }
        }
    }

    #[test]
    fn application_id_alias_beats_popular_description_mentions() {
        let mut apps = Apps::new();
        for (id, name, description, downloads) in [
            (
                "org.gimp.GIMP",
                "GNU Image Manipulation Program",
                "Photo editor",
                1,
            ),
            (
                "org.example.Cleaner",
                "Cleaner",
                "Cleans GIMP files",
                1_000_000,
            ),
        ] {
            apps.insert(
                AppId::new(id),
                vec![AppEntry {
                    backend_name: "flatpak-system",
                    info: Arc::new(AppInfo {
                        name: name.to_string(),
                        description: description.to_string(),
                        monthly_downloads: downloads,
                        ..AppInfo::default()
                    }),
                    installed: false,
                }],
            );
        }
        let results = search_results(
            &apps,
            &Backends::new(),
            &HashMap::new(),
            "noble",
            " gimp ",
            SearchSortMode::Relevance,
            WaylandFilter::All,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, AppId::new("org.gimp.GIMP"));
    }

    #[test]
    fn subcategories_use_metadata_and_keep_unclassified_apps_in_all() {
        let mut apps = Apps::new();
        for (id, categories) in [
            ("strategy", vec!["Game", "StrategyGame"]),
            ("action", vec!["Game", "ActionGame"]),
            ("unclassified", vec!["Game"]),
        ] {
            apps.insert(
                AppId::new(id),
                vec![AppEntry {
                    backend_name: "flatpak-system",
                    info: Arc::new(AppInfo {
                        name: id.to_string(),
                        categories: categories.into_iter().map(str::to_string).collect(),
                        ..AppInfo::default()
                    }),
                    installed: false,
                }],
            );
        }
        let results = categories_results(
            &apps,
            &Backends::new(),
            &HashMap::new(),
            "noble",
            &[Category::StrategyGame],
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].info.name, "strategy");
        let all = categories_results(
            &apps,
            &Backends::new(),
            &HashMap::new(),
            "noble",
            &[Category::Game],
        );
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn unavailable_system_source_falls_back_to_available_flatpak() {
        let info = Arc::new(AppInfo {
            name: "Example".to_string(),
            pkgnames: vec!["example".to_string()],
            origin_opt: Some("zorin".to_string()),
            ..AppInfo::default()
        });
        let id = AppId::new("org.example.App");
        let mut apps = Apps::new();
        apps.insert(
            id.clone(),
            vec![
                AppEntry {
                    backend_name: "packagekit",
                    info: info.clone(),
                    installed: false,
                },
                AppEntry {
                    backend_name: "flatpak-system",
                    info,
                    installed: false,
                },
            ],
        );
        let mut backends = Backends::new();
        backends.insert("packagekit", Arc::new(UnavailableSystemBackend));
        let results = search_results(
            &apps,
            &backends,
            &HashMap::new(),
            "noble",
            "Example",
            SearchSortMode::Relevance,
            WaylandFilter::All,
        );
        assert_eq!(results[0].backend_name(), "flatpak-system");
        let discovery = explore_results_all(&apps, &backends, &HashMap::new(), "noble", 0);
        assert_eq!(
            discovery[&ExplorePage::PopularApps][0].backend_name(),
            "flatpak-system"
        );
        apps.get_mut(&id).unwrap()[0].installed = true;
        let installed = installed_results_data(&apps, &backends, &HashMap::new(), "noble");
        assert_eq!(installed[0].backend_name(), "packagekit");
    }

    #[test]
    fn zorin_origin_is_visible_in_search_and_discovery() {
        let info = AppInfo {
            name: "Zorin App".to_string(),
            origin_opt: Some("zorin".to_string()),
            pkgnames: vec!["zorin-app".to_string()],
            ..AppInfo::default()
        };
        let mut apps = Apps::new();
        apps.insert(
            AppId::new("org.zorin.Example"),
            vec![AppEntry {
                backend_name: "packagekit",
                info: Arc::new(info),
                installed: true,
            }],
        );
        let results = search_results(
            &apps,
            &Backends::new(),
            &HashMap::new(),
            "noble",
            "Zorin",
            SearchSortMode::Relevance,
            WaylandFilter::All,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].backend_name(), "packagekit");
        let discovery = explore_results_all(&apps, &Backends::new(), &HashMap::new(), "noble", 0);
        assert_eq!(discovery[&ExplorePage::PopularApps].len(), 1);
    }

    #[test]
    fn discovery_preserves_popularity_without_external_stats() {
        let results = explore_results_all(
            &sample_apps(),
            &Backends::new(),
            &HashMap::new(),
            "noble",
            0,
        );
        assert_eq!(results[&ExplorePage::PopularApps][0].info.name, "Zulu");
        assert_eq!(results[&ExplorePage::PopularApps][0].weight, -100);
    }

    #[test]
    fn explicit_zero_downloads_override_metadata() {
        let mut stats = HashMap::new();
        stats.insert(AppId::new("org.example.Zulu"), (0, None));
        let results = explore_results_all(&sample_apps(), &Backends::new(), &stats, "noble", 0);
        assert_eq!(results[&ExplorePage::PopularApps][0].info.name, "Alpha");
    }

    #[test]
    fn search_preserves_popularity_without_external_stats() {
        let results = search_results(
            &sample_apps(),
            &Backends::new(),
            &HashMap::new(),
            "noble",
            "",
            SearchSortMode::Relevance,
            WaylandFilter::All,
        );
        assert_eq!(results[0].info.name, "Zulu");
    }
}

fn entry_available(
    backend_name: &str,
    info: &crate::app_info::AppInfo,
    installed: bool,
    backends: &Backends,
) -> bool {
    if installed || backend_name != "packagekit" {
        return true;
    }
    backends
        .get("packagekit")
        .is_none_or(|backend| backend.is_package_available(&info.pkgnames))
}

#[cfg(test)]
mod unified_sort_tests {
    use super::*;
    #[test]
    fn mixed_sources_share_name_and_popularity_sorting() {
        let make = |backend, name: &str, downloads| {
            SearchResult::new(
                backend,
                crate::AppId::new(name),
                None,
                std::sync::Arc::new(crate::AppInfo {
                    name: name.to_string(),
                    monthly_downloads: downloads,
                    ..crate::AppInfo::default()
                }),
                0,
            )
        };
        let mut results = vec![
            make("packagekit", "Zulu", 9),
            make("steam", "Alpha", 0),
            make("flatpak-user", "Bravo", 10),
        ];
        let stats = std::collections::HashMap::new();
        sort_results(&mut results, SearchSortMode::Name, &stats);
        assert_eq!(
            results
                .iter()
                .map(|r| r.info.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Bravo", "Zulu"]
        );
        sort_results(&mut results, SearchSortMode::MostDownloads, &stats);
        assert_eq!(
            results
                .iter()
                .map(|r| r.info.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Bravo", "Zulu", "Alpha"]
        );
    }
}
