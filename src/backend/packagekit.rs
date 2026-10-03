use cosmic::widget;
use packagekit_zbus::{
    PackageKit::PackageKitProxyBlocking,
    Transaction::TransactionProxyBlocking,
    zbus::{blocking::Connection, zvariant},
};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::Write,
    sync::{Arc, Mutex},
    time::Instant,
};

use super::{Backend, Package};
use crate::{AppId, AppInfo, AppUrl, AppstreamCache, GStreamerCodec, Operation, OperationKind};

#[derive(Debug)]
struct TransactionDetails {
    //TODO: more fields: https://www.freedesktop.org/software/PackageKit/gtk-doc/Transaction.html#Transaction::Details
    package_id: String,
    summary: String,
    description: String,
    url: String,
}

#[allow(dead_code)]
#[derive(Debug)]
struct TransactionPackage {
    info: u32,
    package_id: String,
    summary: String,
}

// Use PackageKit's actual update candidates, not installed-version IDs or
// Resolve(NotInstalled), which excludes the packages the user wants to update.
fn update_package_ids(
    requested: &[&str],
    available: &[TransactionPackage],
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for package in available {
        let name = package.package_id.split(';').next().unwrap_or_default();
        if requested.contains(&name) && !ids.contains(&package.package_id) {
            ids.push(package.package_id.clone());
        }
    }
    if ids.is_empty() {
        return Err(
            "No pending updates were found for the selected packages. Refresh the updates list and try again."
                .into(),
        );
    }
    Ok(ids)
}

struct TransactionProgress {
    package_id: String,
    status: u32,
    percentage: u32,
}

/// A PackageKit transaction whose signals are subscribed to before it can start.
/// Subscribing after the first method call loses fast `ErrorCode` and `Finished`
/// signals (for example a refused authorization), which left the UI at 0% forever.
/// Always consume `signals` (see `transaction_handle`): an unread subscription fills
/// up on busy transactions and blocks the shared D-Bus connection.
struct PkTransaction<'a> {
    proxy: TransactionProxyBlocking<'a>,
    signals: Box<dyn Iterator<Item = Arc<packagekit_zbus::zbus::Message>>>,
}

impl<'a> std::ops::Deref for PkTransaction<'a> {
    type Target = TransactionProxyBlocking<'a>;

    fn deref(&self) -> &Self::Target {
        &self.proxy
    }
}

fn transaction_handle(
    tx: PkTransaction,
    mut on_progress: impl FnMut(u32, TransactionProgress),
) -> Result<(Vec<TransactionDetails>, Vec<TransactionPackage>), Box<dyn Error>> {
    let mut details = Vec::new();
    let mut packages = Vec::new();
    let PkTransaction { proxy: tx, signals } = tx;
    for signal in signals {
        if let Some(member) = signal.member() {
            match member.as_str() {
                "Details" => {
                    let map = signal.body::<HashMap<String, zvariant::Value>>()?;

                    let get_string = |key: &str| -> Option<String> {
                        match map.get(key) {
                            Some(zvariant::Value::Str(str)) => Some(str.to_string()),
                            unknown => {
                                log::warn!(
                                    "failed to find string for key {:?} in packagekit Details: found {:?} instead",
                                    key,
                                    unknown
                                );
                                None
                            }
                        }
                    };

                    let Some(package_id) = get_string("package-id") else {
                        continue;
                    };
                    let summary = get_string("summary").unwrap_or_default();
                    let description = get_string("description").unwrap_or_default();
                    let url = get_string("url").unwrap_or_default();
                    details.push(TransactionDetails {
                        package_id,
                        summary,
                        description,
                        url,
                    });
                }
                "ErrorCode" => {
                    // https://www.freedesktop.org/software/PackageKit/gtk-doc/Transaction.html#Transaction::ErrorCode
                    let (code, details) = signal.body::<(u32, String)>()?;
                    if code != 48 {
                        return Err(format!("{details} (code {code})").into());
                    }
                }
                "ItemProgress" => {
                    // https://www.freedesktop.org/software/PackageKit/gtk-doc/Transaction.html#Transaction::ItemProgress
                    let (package_id, status, percentage) = signal.body::<(String, u32, u32)>()?;
                    let total_percentage = tx.percentage().unwrap_or(percentage);
                    on_progress(
                        total_percentage,
                        TransactionProgress {
                            package_id,
                            status,
                            percentage,
                        },
                    )
                }
                "Package" => {
                    // https://www.freedesktop.org/software/PackageKit/gtk-doc/Transaction.html#Transaction::Package
                    let (info, package_id, summary) = signal.body::<(u32, String, String)>()?;
                    packages.push(TransactionPackage {
                        info,
                        package_id,
                        summary,
                    });
                }
                "Finished" => {
                    break;
                }
                _ => {
                    log::warn!("unknown signal {}", member);
                }
            }
        }
    }
    Ok((details, packages))
}

// https://lazka.github.io/pgi-docs/PackageKitGlib-1.0/enums.html#PackageKitGlib.FilterEnum
#[repr(u64)]
enum FilterKind {
    None = 1 << 1,
    Installed = 1 << 2,
    NotInstalled = 1 << 3,
    Newest = 1 << 16,
    Arch = 1 << 18,
}

#[allow(dead_code)]
#[repr(u64)]
enum TransactionFlag {
    None = 1 << 0,
    OnlyTrusted = 1 << 1,
    AllowReinstall = 1 << 4,
    AllowDowngrade = 1 << 6,
}

/// On-disk copy of the available package names. PackageKit needs several seconds to
/// enumerate them, so the copy is reused until the apt metadata changes.
#[derive(bitcode::Encode, bitcode::Decode)]
struct AvailableNamesCache {
    fingerprint: u64,
    names: Vec<String>,
}

const APT_LISTS_DIR: &str = "/var/lib/apt/lists";

/// Changes whenever `apt update` replaces repository metadata or sources change.
fn apt_lists_fingerprint() -> Option<u64> {
    let mut newest = 0_u64;
    let mut count = 0_u64;
    for entry in std::fs::read_dir(APT_LISTS_DIR).ok()? {
        let modified = entry.ok()?.metadata().ok()?.modified().ok()?;
        let secs = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        newest = newest.max(secs);
        count += 1;
    }
    // Without any lists the fingerprint would not prove anything.
    (count > 0).then_some(newest ^ (count << 40))
}

fn available_names_cache_path() -> Option<std::path::PathBuf> {
    Some(
        dirs::cache_dir()?
            .join(crate::constants::CACHE_DIR)
            .join("packagekit-available.bitcode"),
    )
}

fn load_available_names(fingerprint: u64) -> Option<HashSet<String>> {
    let bytes = std::fs::read(available_names_cache_path()?).ok()?;
    let cache = bitcode::decode::<AvailableNamesCache>(&bytes).ok()?;
    (cache.fingerprint == fingerprint && !cache.names.is_empty())
        .then(|| cache.names.into_iter().collect())
}

fn save_available_names(fingerprint: u64, names: &HashSet<String>) {
    let Some(path) = available_names_cache_path() else {
        return;
    };
    let cache = AvailableNamesCache {
        fingerprint,
        names: names.iter().cloned().collect(),
    };
    crate::utils::write_cache_file(&path, &bitcode::encode(&cache));
}

#[derive(Debug)]
enum PackageAvailability {
    Known(HashSet<String>),
    Unknown,
}

#[derive(Debug)]
pub struct Packagekit {
    connection: Connection,
    appstream_caches: Vec<AppstreamCache>,
    available_packages_cache: Arc<Mutex<Option<PackageAvailability>>>,
}

impl Packagekit {
    pub fn new(locale: &str) -> Result<Self, Box<dyn Error>> {
        //TODO: cache more zbus stuff?
        let connection = Connection::system()?;
        let source_id = "packagekit";
        //TODO: translate?
        let source_name = crate::fl!("system-packages");
        Ok(Self {
            connection,
            appstream_caches: vec![AppstreamCache::system(
                source_id.to_string(),
                source_name,
                locale,
            )],
            available_packages_cache: Arc::new(Mutex::new(None)),
        })
    }

    /// Build a cache of available package names from PackageKit
    fn build_available_packages_cache(&self) -> Result<HashSet<String>, Box<dyn Error>> {
        log::info!("Building available packages cache from PackageKit...");
        let start = Instant::now();

        // The transaction is already subscribed to its signals, so a fast cached
        // transaction cannot finish before we listen. Use exactly that one
        // subscription: a second, unread one would fill up while get_packages emits
        // tens of thousands of signals and stall the whole D-Bus connection.
        let PkTransaction { proxy: tx, signals } = self.transaction()?;
        tx.get_packages(FilterKind::Arch as u64)?;
        // Availability needs raw names only, without app cards or icon loading.
        let mut available = HashSet::new();
        for signal in signals {
            let Some(member) = signal.member() else {
                continue;
            };
            match member.as_str() {
                "Package" => {
                    let (_, package_id, _) = signal.body::<(u32, String, String)>()?;
                    if let Some(name) = package_id.split(';').next() {
                        available.insert(name.to_string());
                    }
                }
                "ErrorCode" => {
                    let (code, details) = signal.body::<(u32, String)>()?;
                    return Err(format!("{details} (code {code})").into());
                }
                "Finished" => break,
                _ => {}
            }
        }

        log::info!(
            "Built available packages cache with {} packages in {:?}",
            available.len(),
            start.elapsed()
        );
        Ok(available)
    }

    /// Check if a package is available for installation
    pub fn is_package_available(&self, pkgnames: &[String]) -> bool {
        // Lazy-load cache on first use
        let mut cache = self.available_packages_cache.lock().unwrap();
        if cache.is_none() {
            let fingerprint = apt_lists_fingerprint();
            if let Some(names) = fingerprint.and_then(load_available_names) {
                log::info!("loaded {} available package names from disk", names.len());
                *cache = Some(PackageAvailability::Known(names));
            }
        }
        if cache.is_none() {
            let fingerprint = apt_lists_fingerprint();
            match self.build_available_packages_cache() {
                Ok(c) => {
                    if let Some(fingerprint) = fingerprint {
                        save_available_names(fingerprint, &c);
                    }
                    *cache = Some(PackageAvailability::Known(c));
                }
                Err(e) => {
                    log::error!("Failed to build available packages cache: {}", e);
                    // Availability is unknown, not unavailable. Cache the failure so
                    // browsing does not repeat a failed transaction for every app.
                    *cache = Some(PackageAvailability::Unknown);
                }
            }
        }

        // Check if any of the package names are available
        match cache.as_ref() {
            Some(PackageAvailability::Known(available)) => {
                pkgnames.iter().any(|name| available.contains(name))
            }
            _ => true,
        }
    }

    fn transaction(&self) -> Result<PkTransaction<'_>, Box<dyn Error>> {
        //TODO: use async?
        let pk = PackageKitProxyBlocking::new(&self.connection)?;
        //TODO: set locale?
        let tx_path = pk.create_transaction()?;
        let tx = TransactionProxyBlocking::builder(&self.connection)
            .destination("org.freedesktop.PackageKit")?
            .path(tx_path)?
            .build()?;
        // Subscribe before any method can start the transaction.
        let signals = tx.receive_all_signals()?;
        Ok(PkTransaction {
            proxy: tx,
            signals: Box::new(signals),
        })
    }

    fn package_transaction(&self, tx: PkTransaction) -> Result<Vec<Package>, Box<dyn Error>> {
        let appstream_cache = &self.appstream_caches[0];

        let (tx_details, tx_packages) = transaction_handle(tx, |_, _| {})?;

        let mut system_packages = Vec::new();
        let mut packages = Vec::new();

        for tx_detail in tx_details {
            //TODO: this is a hack to handle file details like they are packages
            let mut parts = tx_detail.package_id.split(';');
            let Some(package_name) = parts.next() else {
                continue;
            };
            let version_opt = parts.next();
            let _architecture_opt = parts.next();

            let data = parts.next().unwrap_or("");
            let mut data_parts = data.split(':');
            let _status_opt = data_parts.next();
            let _origin_opt = data_parts.next();

            //TODO: translate
            packages.push(Package {
                id: AppId::new(package_name),
                icon: widget::icon::from_name("package-x-generic")
                    .size(128)
                    .handle(),
                //TODO: fill in more AppInfo fields
                info: Arc::new(AppInfo {
                    source_id: appstream_cache.source_id.clone(),
                    source_name: appstream_cache.source_name.clone(),
                    name: package_name.to_string(),
                    summary: tx_detail.summary.clone(),
                    description: tx_detail.description.clone(),
                    pkgnames: vec![package_name.to_string()],
                    urls: if !tx_detail.url.is_empty() {
                        vec![AppUrl::Homepage(tx_detail.url.to_string())]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                }),
                version: version_opt.unwrap_or("").to_string(),
                extra: HashMap::new(),
            });
        }

        for tx_package in tx_packages {
            let mut parts = tx_package.package_id.split(';');
            let Some(package_name) = parts.next() else {
                continue;
            };
            let version_opt = parts.next();
            let _architecture_opt = parts.next();

            let data = parts.next().unwrap_or("");
            let mut data_parts = data.split(':');
            let _status_opt = data_parts.next();
            let _origin_opt = data_parts.next();

            match appstream_cache.pkgnames.get(package_name) {
                Some(ids) => {
                    for id in ids.iter() {
                        match appstream_cache.infos.get(id) {
                            Some(info) => {
                                packages.push(Package {
                                    id: id.clone(),
                                    icon: appstream_cache.icon(info),
                                    info: info.clone(),
                                    version: version_opt.unwrap_or("").to_string(),
                                    extra: HashMap::new(),
                                });
                            }
                            None => {
                                log::warn!("failed to find info {:?}", id);
                            }
                        }
                    }
                }
                None => {
                    // Ignore packages with no components
                    log::debug!("no components for package {}", package_name);
                    system_packages.push((
                        package_name.to_string(),
                        version_opt.unwrap_or("").to_string(),
                    ));
                }
            }
        }
        if !system_packages.is_empty() {
            let name = "System Packages".to_string();
            let summary = format!(
                "{} package{}",
                system_packages.len(),
                if system_packages.len() == 1 { "" } else { "s" }
            );
            let mut description = String::new();
            let mut pkgnames = Vec::with_capacity(system_packages.len());
            for (package_name, version) in system_packages {
                let _ = writeln!(description, " * {}: {}", package_name, version);
                pkgnames.push(package_name);
            }
            //TODO: translate
            packages.push(Package {
                id: AppId::system(),
                icon: widget::icon::from_name("package-x-generic")
                    .size(128)
                    .handle(),
                //TODO: fill in more AppInfo fields
                info: Arc::new(AppInfo {
                    source_id: appstream_cache.source_id.clone(),
                    source_name: appstream_cache.source_name.clone(),
                    name,
                    summary,
                    description,
                    pkgnames,
                    ..Default::default()
                }),
                version: String::new(),
                extra: HashMap::new(),
            });
        }
        Ok(packages)
    }
}

impl Backend for Packagekit {
    fn load_caches(&mut self, refresh: bool) -> Result<(), Box<dyn Error>> {
        if refresh {
            let tx = self.transaction()?;
            tx.set_hints(&["interactive=true", "cache-age=300"])?;
            tx.refresh_cache(false)?;
            // Invalidate available packages cache
            *self.available_packages_cache.lock().unwrap() = None;
        }

        for appstream_cache in self.appstream_caches.iter_mut() {
            appstream_cache.reload();
        }
        Ok(())
    }

    fn info_caches(&self) -> &[AppstreamCache] {
        &self.appstream_caches
    }

    fn installed(&self) -> Result<Vec<Package>, Box<dyn Error>> {
        let tx = self.transaction()?;
        tx.get_packages(FilterKind::Installed as u64)?;
        self.package_transaction(tx)
    }

    fn updates(&self) -> Result<Vec<Package>, Box<dyn Error>> {
        let tx = self.transaction()?;
        tx.get_updates(FilterKind::None as u64)?;
        self.package_transaction(tx)
    }

    fn file_packages(&self, path: &str) -> Result<Vec<Package>, Box<dyn Error>> {
        let tx = self.transaction()?;
        tx.get_details_local(&[path])?;
        let mut packages = self.package_transaction(tx)?;
        for package in packages.iter_mut() {
            let info = Arc::make_mut(&mut package.info);
            info.package_paths.push(path.to_string());
        }
        Ok(packages)
    }

    fn gstreamer_packages(
        &self,
        gstreamer_codec: &GStreamerCodec,
    ) -> Result<Vec<Package>, Box<dyn Error>> {
        // Packagekit provides looks like gstreamer1.0(decoder-video/x-h264)
        //TODO: truncate version ending in .0? gstreamer1.0-packagekit does this but it does not appear to be required
        let provides = format!(
            "gstreamer{}({})",
            gstreamer_codec.version, gstreamer_codec.type_name
        );
        let tx = self.transaction()?;
        tx.what_provides(
            FilterKind::Newest as u64 | FilterKind::Arch as u64,
            &[&provides],
        )?;
        let (_tx_details, tx_packages) = transaction_handle(tx, |_, _| {})?;

        // Convert packages to details in order to show more information
        let mut package_ids = Vec::with_capacity(tx_packages.len());
        for tx_package in tx_packages.iter() {
            package_ids.push(tx_package.package_id.as_str());
        }
        let tx = self.transaction()?;
        tx.get_details(&package_ids)?;
        self.package_transaction(tx)
    }

    fn operation(
        &self,
        op: &Operation,
        mut f: Box<dyn FnMut(f32) + 'static>,
    ) -> Result<(), Box<dyn Error>> {
        let mut package_names = Vec::new();
        let mut package_paths = Vec::new();
        for info in op.infos.iter() {
            for pkgname in &info.pkgnames {
                package_names.push(pkgname.as_str());
            }
            for package_path in &info.package_paths {
                package_paths.push(package_path.as_str());
            }
        }
        if package_names.is_empty() {
            return Err(format!("{:?} missing package name", op.package_ids).into());
        }
        let (_tx_details, tx_packages) = {
            let tx = self.transaction()?;
            log::info!("resolve packages for {:?}", package_names);
            let filter = match &op.kind {
                OperationKind::Install => {
                    FilterKind::NotInstalled as u64
                        | FilterKind::Newest as u64
                        | FilterKind::Arch as u64
                }
                OperationKind::Uninstall { .. } => FilterKind::Installed as u64,
                // Other operations not supported
                _ => 0,
            };
            if matches!(op.kind, OperationKind::Update) {
                tx.get_updates(FilterKind::None as u64)?;
            } else {
                tx.resolve(filter, &package_names)?;
            }
            transaction_handle(tx, |_, _| {})?
        };
        let selected_updates;
        let package_ids: Vec<&str> = if matches!(op.kind, OperationKind::Update) {
            selected_updates = update_package_ids(&package_names, &tx_packages)?;
            selected_updates.iter().map(String::as_str).collect()
        } else {
            tx_packages
                .iter()
                .map(|package| package.package_id.as_str())
                .collect()
        };
        let tx = self.transaction()?;
        tx.set_hints(&["interactive=true"])?;
        match &op.kind {
            OperationKind::Install => {
                if !package_paths.is_empty() {
                    log::info!("installing package files {:?}", package_paths);
                    //TODO: transaction flags
                    tx.install_files(0, &package_paths)?;
                } else {
                    log::info!("installing packages {:?}", package_ids);
                    //TODO: transaction flags
                    tx.install_packages(TransactionFlag::OnlyTrusted as u64, &package_ids)?;
                }
            }
            OperationKind::Uninstall { purge_data } => {
                log::info!(
                    "uninstalling packages {:?} (purge_data: {})",
                    package_ids,
                    purge_data
                );
                if *purge_data {
                    log::warn!(
                        "PackageKit backend does not fully support purging configuration files. \
                        Only the package will be removed. Configuration files may remain in user directories."
                    );
                }
                //TODO: transaction flags?
                //TODO: investigate if we can detect package managers like dnf, apt, etc
                // and use purge-specific functionality
                tx.remove_packages(0, &package_ids, true, true)?;
            }
            OperationKind::Update => {
                log::info!("updating packages {:?}", package_ids);
                //TODO: transaction flags?
                tx.update_packages(TransactionFlag::OnlyTrusted as u64, &package_ids)?;
            }
            OperationKind::RepositoryAdd { .. } => {
                return Err("packagekit backend does not support adding repositories".into());
            }
            OperationKind::RepositoryRemove { .. } => {
                return Err("packagekit backend does not support removing repositories".into());
            }
        }
        let _tx_packages = transaction_handle(tx, |total_percentage, progress| {
            log::info!(
                "{}%: {} {} {}%",
                total_percentage,
                progress.package_id,
                progress.status,
                progress.percentage
            );
            f(total_percentage as f32);
        })?;
        Ok(())
    }

    fn is_package_available(&self, pkgnames: &[String]) -> bool {
        self.is_package_available(pkgnames)
    }
}

#[cfg(test)]
mod update_tests {
    use super::{TransactionPackage, update_package_ids};

    fn package(id: &str) -> TransactionPackage {
        TransactionPackage {
            info: 0,
            package_id: id.into(),
            summary: String::new(),
        }
    }

    #[test]
    fn updates_use_available_versions_and_exclude_unselected_packages() {
        let available = vec![
            package("kompas;0.2;amd64;updates"),
            package("other;3;amd64;updates"),
        ];
        assert_eq!(
            update_package_ids(&["kompas"], &available).unwrap(),
            vec!["kompas;0.2;amd64;updates"]
        );
    }

    #[test]
    fn updates_preserve_installed_multiarch_candidates_without_duplicates() {
        let available = vec![
            package("libexample;2;amd64;updates"),
            package("libexample;2;i386;updates"),
            package("libexample;2;amd64;updates"),
        ];
        assert_eq!(
            update_package_ids(&["libexample"], &available)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn stale_update_selection_reports_no_pending_updates() {
        assert!(update_package_ids(&["kompas"], &[package("other;3;amd64;updates")]).is_err());
    }
}

#[cfg(test)]
mod availability_cache_tests {
    use super::AvailableNamesCache;

    #[test]
    fn available_names_round_trip_through_bitcode() {
        let cache = AvailableNamesCache {
            fingerprint: 42,
            names: vec!["gimp".into(), "kompas".into()],
        };
        let decoded = bitcode::decode::<AvailableNamesCache>(&bitcode::encode(&cache)).unwrap();
        assert_eq!(decoded.fingerprint, 42);
        assert_eq!(decoded.names, ["gimp", "kompas"]);
        assert!(bitcode::decode::<AvailableNamesCache>(b"not a cache").is_err());
    }
}
