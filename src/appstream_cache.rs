use appstream::{
    Component, Image, MarkupTranslatableString, ParseError, Release, Screenshot,
    enums::{
        ComponentKind, Icon, ImageKind, Launchable, ProjectUrl, Provide, ReleaseKind,
        ReleaseUrgency,
    },
    url::Url,
    xmltree,
};
use cosmic::widget;
use flate2::read::GzDecoder;
use rayon::prelude::*;
use serde::Deserialize;
use std::{
    cmp,
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime},
};

use crate::{AppIcon, AppId, AppInfo, app_info::WaylandCompatibility, stats};

#[cfg(test)]
mod appstream_tests;

type ParsedData = (Option<String>, Vec<(AppId, Arc<AppInfo>)>, Vec<Component>);

/// Extract Wayland compatibility bitcode from AppStream XML custom fields.
///
/// This function looks for a custom value with key "wayland_compat" in XML
/// and decodes it from hex format to a WaylandCompatibility struct.
///
/// # Example XML
/// ```xml
/// <custom>
///   <value key="wayland_compat">0x0A</value>
/// </custom>
/// ```
fn extract_wayland_bitcode(element: &xmltree::Element) -> Option<WaylandCompatibility> {
    // Find custom element
    let custom_elem = element.get_child("custom")?;

    // Find all value elements within custom
    for node in custom_elem.children.iter() {
        if let xmltree::XMLNode::Element(value_elem) = node {
            if value_elem.name == "value" {
                // Check if this is the wayland_compat key
                if let Some(key) = value_elem.attributes.get("key") {
                    if key == "wayland_compat" {
                        // Get the text content
                        if let Some(xmltree::XMLNode::Text(text)) = value_elem.children.first() {
                            // Parse hex string (e.g., "0x0A")
                            let text = text.trim();
                            if let Some(hex_str) = text.strip_prefix("0x") {
                                if let Ok(bitcode) = u8::from_str_radix(hex_str, 16) {
                                    log::debug!("Found wayland_compat bitcode: 0x{:02X}", bitcode);
                                    return Some(WaylandCompatibility::decode_bitcode(bitcode));
                                } else {
                                    log::warn!("Failed to parse wayland_compat bitcode: {}", text);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

const PREFIXES: &[&str] = &["/usr/share", "/var/lib", "/var/cache"];
const CATALOGS: &[&str] = &["swcatalog", "app-info"];

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    bitcode::Decode,
    bitcode::Encode,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct AppstreamCacheTag {
    /// When the file was last modified in seconds from unix epoch
    pub modified: u64,
    /// Size of file in bytes
    pub size: u64,
}

#[derive(Debug, Default, bitcode::Decode, bitcode::Encode)]
pub struct AppstreamCache {
    pub source_id: String,
    pub source_name: String,
    // Uses btreemap for stable sort order
    pub path_tags: BTreeMap<String, AppstreamCacheTag>,
    pub icons_paths: Vec<String>,
    pub locale: String,
    pub infos: HashMap<AppId, Arc<AppInfo>>,
    pub pkgnames: HashMap<String, HashSet<AppId>>,
    pub addons: HashMap<AppId, Vec<AppId>>,
}

/// A repeated header key that does not identify the repository or its media.
fn is_harmless_duplicate_header_key(error: &serde_yaml::Error) -> bool {
    let message = error.to_string();
    message.contains("duplicate entry")
        && !message.contains("\"Origin\"")
        && !message.contains("\"MediaBaseUrl\"")
}

impl AppstreamCache {
    /// Get cache for specified appstream data sources
    pub fn new(
        source_id: String,
        source_name: String,
        paths: Vec<PathBuf>,
        icons_paths: Vec<String>,
        locale: &str,
    ) -> Self {
        let mut cache = Self {
            source_id,
            source_name,
            icons_paths,
            locale: locale.to_string(),
            ..Self::default()
        };

        for path in paths.iter() {
            let canonical = match fs::canonicalize(path) {
                Ok(pathbuf) => match pathbuf.into_os_string().into_string() {
                    Ok(ok) => ok,
                    Err(os_string) => {
                        log::error!("failed to convert {:?} to string", os_string);
                        continue;
                    }
                },
                Err(err) => {
                    log::error!("failed to canonicalize {:?}: {}", path, err);
                    continue;
                }
            };

            let metadata = match fs::metadata(&canonical) {
                Ok(ok) => ok,
                Err(err) => {
                    log::error!("failed to read metadata of {:?}: {}", canonical, err);
                    continue;
                }
            };

            let modified = match metadata.modified() {
                Ok(system_time) => match system_time.duration_since(SystemTime::UNIX_EPOCH) {
                    Ok(duration) => duration.as_secs(),
                    Err(err) => {
                        log::error!(
                            "failed to convert modified time of {:?} to unix epoch: {}",
                            canonical,
                            err
                        );
                        continue;
                    }
                },
                Err(err) => {
                    log::error!("failed to read modified time of {:?}: {}", canonical, err);
                    continue;
                }
            };

            let size = metadata.len();

            cache
                .path_tags
                .insert(canonical, AppstreamCacheTag { modified, size });
        }

        cache
    }

    /// Get cache for system appstream data sources
    pub fn system(source_id: String, source_name: String, locale: &str) -> Self {
        let mut paths = Vec::new();
        let mut icons_paths = Vec::new();
        //TODO: get using xdg dirs?
        for prefix in PREFIXES {
            let prefix_path = Path::new(prefix);
            if !prefix_path.is_dir() {
                continue;
            }

            for catalog in CATALOGS {
                let catalog_path = prefix_path.join(catalog);
                if !catalog_path.is_dir() {
                    continue;
                }

                for format in &["xml", "yaml"] {
                    let format_path = catalog_path.join(format);
                    if !format_path.is_dir() {
                        continue;
                    }

                    let readdir = match fs::read_dir(&format_path) {
                        Ok(ok) => ok,
                        Err(err) => {
                            log::error!("failed to read directory {:?}: {}", format_path, err);
                            continue;
                        }
                    };

                    for entry_res in readdir {
                        let entry = match entry_res {
                            Ok(ok) => ok,
                            Err(err) => {
                                log::error!(
                                    "failed to read entry in directory {:?}: {}",
                                    format_path,
                                    err
                                );
                                continue;
                            }
                        };

                        paths.push(entry.path());
                    }
                }

                let icons_path = catalog_path.join("icons");
                if icons_path.is_dir() {
                    match icons_path.into_os_string().into_string() {
                        Ok(ok) => icons_paths.push(ok),
                        Err(os_string) => {
                            log::error!("failed to convert {:?} to string", os_string)
                        }
                    }
                }
            }
        }

        AppstreamCache::new(source_id, source_name, paths, icons_paths, locale)
    }

    /// Directory where cache should be stored
    fn cache_dir(&self, cache_name: &str) -> Option<PathBuf> {
        dirs::cache_dir().map(|x| x.join(crate::constants::CACHE_DIR).join(cache_name))
    }

    /// Cache version includes original product release-date metadata.
    fn cache_filename() -> &'static str {
        "appstream_cache_v3.bitcode"
    }

    /// Remove all files from cache not matching filename
    pub fn clean_cache(&self, cache_name: &str) {
        let start = Instant::now();

        let cache_dir = match self.cache_dir(cache_name) {
            Some(some) => some,
            None => {
                log::warn!("failed to find cache directory");
                return;
            }
        };

        if !cache_dir.is_dir() {
            match fs::create_dir_all(&cache_dir) {
                Ok(()) => {}
                Err(err) => {
                    log::warn!("failed to create cache directory {:?}: {}", cache_dir, err);
                    return;
                }
            }
        }

        let read_dir = match fs::read_dir(&cache_dir) {
            Ok(ok) => ok,
            Err(err) => {
                log::warn!("failed to read cache directory {:?}: {}", cache_dir, err);
                return;
            }
        };

        for entry_res in read_dir {
            let entry = match entry_res {
                Ok(ok) => ok,
                Err(err) => {
                    log::warn!(
                        "failed to read entry in cache directory {:?}: {}",
                        cache_dir,
                        err
                    );
                    continue;
                }
            };

            let path = entry.path();
            if path.is_dir() {
                log::warn!("unexpected directory in cache: {:?}", path);
                continue;
            }

            if entry.file_name() != Self::cache_filename() {
                match fs::remove_file(&path) {
                    Ok(()) => {
                        log::info!("removed outdated cache file {:?}", entry.path());
                    }
                    Err(err) => {
                        log::info!(
                            "failed to remove outdated cache file {:?}: {}",
                            entry.path(),
                            err
                        );
                    }
                }
            }
        }

        let duration = start.elapsed();
        log::info!("cleaned cache {:?} in {:?}", cache_name, duration);
    }

    /// Reload from cache, returns true if loaded and false if out of date
    //TODO: return errors instead of handling them internally?
    pub fn load_cache(&mut self, cache_name: &str) -> bool {
        let start = Instant::now();

        let cache_dir = match self.cache_dir(cache_name) {
            Some(some) => some,
            None => {
                log::warn!("failed to find cache directory");
                return false;
            }
        };
        let cache_path = cache_dir.join(Self::cache_filename());

        let data = match fs::read(&cache_path) {
            Ok(ok) => ok,
            Err(err) => {
                log::warn!("failed to read cache {:?}: {}", cache_path, err);
                return false;
            }
        };

        let cache = match bitcode::decode::<Self>(&data) {
            Ok(ok) => ok,
            Err(err) => {
                log::warn!("failed to decode cache {:?}: {}", cache_name, err);
                return false;
            }
        };

        if cache.path_tags != self.path_tags {
            log::info!("cache {:?} path tags mismatch, needs refresh", cache_name);
            return false;
        }

        //TODO: icons_paths intentionally ignored, should it be?

        if cache.locale != self.locale {
            log::info!("cache {:?} locale mismatch, needs refresh", cache_name);
            return false;
        }

        // Everything matches, copy infos and pkgnames
        self.infos = cache.infos;
        self.pkgnames = cache.pkgnames;
        self.addons = cache.addons;

        let duration = start.elapsed();
        log::info!("loaded cache {:?} in {:?}", cache_name, duration);
        true
    }

    /// Save to cache
    //TODO: return errors instead of handling them internally?
    pub fn save_cache(&self, cache_name: &str) {
        let start = Instant::now();

        let bitcode = bitcode::encode::<Self>(self);

        let cache_dir = match self.cache_dir(cache_name) {
            Some(some) => some,
            None => {
                log::warn!("failed to find user cache directory");
                return;
            }
        };
        let cache_path = cache_dir.join(Self::cache_filename());

        match atomicwrites::AtomicFile::new(
            &cache_path,
            atomicwrites::OverwriteBehavior::AllowOverwrite,
        )
        .write(|file| file.write_all(&bitcode))
        {
            Ok(()) => {}
            Err(err) => {
                log::warn!("failed to write cache {:?}: {}", cache_path, err);
                return;
            }
        }

        let duration = start.elapsed();
        log::info!("saved cache {:?} in {:?}", cache_name, duration);
    }

    /// Either load from cache or load from originals. Cache is cleaned before loading and saved after.
    pub fn reload(&mut self) {
        let source_id = self.source_id.clone();
        self.clean_cache(&source_id);
        if !self.load_cache(&source_id) {
            self.load_original();
            self.save_cache(&source_id);
        }

        // DEBUG: Inject mock bitcode data if COSMIC_STORE_INJECT_BITCODE is set
        if std::env::var("COSMIC_STORE_INJECT_BITCODE").is_ok() {
            self.inject_mock_bitcode_data();
        }
    }

    /// DEBUG ONLY: Inject mock bitcode data into existing apps for testing
    /// This simulates receiving bitcode data from Flathub without needing actual AppStream updates
    fn inject_mock_bitcode_data(&mut self) {
        use crate::app_info::WaylandCompatibility;

        log::info!("🧪 DEBUG MODE: Injecting mock bitcode data into existing apps");

        // Map of app IDs to their bitcode values
        let bitcode_map: Vec<(&str, u8)> = vec![
            // GNOME apps - GTK4 + Native + Low (0x0A)
            ("org.gnome.Epiphany", 0x0A),
            ("org.gnome.Nautilus", 0x0A),
            ("org.gnome.TextEditor", 0x0A),
            ("org.gnome.Calculator", 0x0A),
            ("org.gnome.Calendar", 0x0A),
            ("org.gnome.Contacts", 0x0A),
            ("org.gnome.Weather", 0x0A),
            ("org.gnome.Maps", 0x0A),
            ("org.gnome.Cheese", 0x0A),
            ("org.gnome.Evince", 0x0A),
            // KDE apps - Qt6 + Native + Medium (0x52)
            ("org.kde.kate", 0x52),
            ("org.kde.okular", 0x52),
            ("org.kde.krita", 0x52),
            ("org.kde.kdenlive", 0x52),
            ("org.kde.dolphin", 0x52),
            ("org.kde.konsole", 0x52),
            // Electron apps - Electron + Native + High (0x96)
            ("com.brave.Browser", 0x96),
            ("com.visualstudio.code", 0x96),
            ("com.discordapp.Discord", 0x96),
            ("com.slack.Slack", 0x96),
            ("org.signal.Signal", 0x96),
            // GTK3 apps - GTK3 + Native + Low (0x06)
            ("org.gimp.GIMP", 0x06),
            ("org.inkscape.Inkscape", 0x06),
            ("org.audacityteam.Audacity", 0x06),
            // Qt5 apps - Qt5 + Native + Medium (0x4E)
            ("org.videolan.VLC", 0x4E),
            ("org.qbittorrent.qBittorrent", 0x4E),
        ];

        let mut injected_count = 0;
        for (app_id_str, bitcode) in bitcode_map {
            let app_id = AppId::new(app_id_str);
            if let Some(info_arc) = self.infos.get_mut(&app_id) {
                // We need to modify the Arc<AppInfo>, so we'll create a new one
                if let Some(info) = Arc::get_mut(info_arc) {
                    info.wayland_compat = Some(WaylandCompatibility::decode_bitcode(bitcode));
                    log::debug!(
                        "  ✅ Injected bitcode 0x{:02X} into {}",
                        bitcode,
                        app_id_str
                    );
                    injected_count += 1;
                } else {
                    // Arc has multiple references, need to clone and modify
                    let mut new_info = (**info_arc).clone();
                    new_info.wayland_compat = Some(WaylandCompatibility::decode_bitcode(bitcode));
                    *info_arc = Arc::new(new_info);
                    log::debug!(
                        "  ✅ Injected bitcode 0x{:02X} into {} (cloned)",
                        bitcode,
                        app_id_str
                    );
                    injected_count += 1;
                }
            }
        }

        log::info!("🧪 Injected bitcode data into {} apps", injected_count);
    }

    pub fn icon_path(
        &self,
        origin_opt: Option<&str>,
        name: &str,
        width_opt: Option<u32>,
        height_opt: Option<u32>,
        scale_opt: Option<u32>,
    ) -> Option<PathBuf> {
        //TODO: what to do with no origin?
        let origin = origin_opt?;
        //TODO: what to do with no width or height?
        let width = width_opt?;
        let height = height_opt?;
        let size = match scale_opt {
            //TODO: should a scale of 0 or 1 not add @scale?
            Some(scale) => format!("{}x{}@{}", width, height, scale),
            None => format!("{}x{}", width, height),
        };

        for icons_path in self.icons_paths.iter() {
            let icon_path = Path::new(icons_path).join(origin).join(&size).join(name);
            if icon_path.is_file() {
                return Some(icon_path);
            }
        }

        //TODO: smarter removal of .desktop
        let fallback_name = name.replace(".desktop", "");
        for icons_path in self.icons_paths.iter() {
            let icon_path = Path::new(icons_path)
                .join(origin)
                .join(&size)
                .join(&fallback_name);
            if icon_path.is_file() {
                return Some(icon_path);
            }
        }

        None
    }

    pub fn icon(&self, info: &AppInfo) -> widget::icon::Handle {
        let mut icon_opt = None;
        let mut cached_size = 0;
        for info_icon in info.icons.iter() {
            //TODO: support other types of icons
            match info_icon {
                AppIcon::Cached(name, width, height, scale) => {
                    let size = cmp::min(width.unwrap_or(0), height.unwrap_or(0));
                    if size < cached_size {
                        // Skip if size is less than cached size
                        continue;
                    }
                    if let Some(icon_path) =
                        self.icon_path(info.origin_opt.as_deref(), name, *width, *height, *scale)
                    {
                        icon_opt = Some(widget::icon::from_path(icon_path));
                        cached_size = size;
                    }
                }
                AppIcon::Stock(stock) => {
                    if cached_size != 0 {
                        // Skip if a cached icon was found
                        continue;
                    }
                    if let Some(icon_path) = widget::icon::from_name(stock.clone()).size(128).path()
                    {
                        icon_opt = Some(widget::icon::from_path(icon_path));
                    }
                }
                AppIcon::Remote(_url, _width, _height, _scale) => {
                    //TODO
                }
                AppIcon::Local(path, width, height, _scale) => {
                    let size = cmp::min(width.unwrap_or(0), height.unwrap_or(0));
                    if size < cached_size {
                        // Skip if size is less than cached size
                        continue;
                    }
                    let icon_path = Path::new(path);
                    if icon_path.is_file() {
                        icon_opt = Some(widget::icon::from_path(icon_path.to_path_buf()));
                        cached_size = size;
                    }
                }
            }
        }

        icon_opt.unwrap_or_else(|| {
            log::debug!("failed to get icon from {:?}", info.icons);
            // Always provide a valid bundled SVG when the host icon theme or
            // repository artwork is missing. Empty image handles can retain stale
            // GPU image layers in the pinned renderer when changing pages.
            crate::icon_cache::icon_cache_handle("store-installed-symbolic", 16)
        })
    }
    pub fn load_original(&mut self) {
        self.infos.clear();
        self.pkgnames.clear();
        self.addons.clear();

        let path_results: Vec<_> = self
            .path_tags
            .par_iter()
            .filter_map(|(path, _tag)| -> Option<ParsedData> {
                let file_name = match Path::new(path).file_name() {
                    Some(file_name_os) => match file_name_os.to_str() {
                        Some(some) => some,
                        None => {
                            log::error!("failed to convert to UTF-8: {:?}", file_name_os);
                            return None;
                        }
                    },
                    None => {
                        log::error!("path has no file name: {:?}", path);
                        return None;
                    }
                };

                let mut file = match fs::File::open(path) {
                    Ok(ok) => ok,
                    Err(err) => {
                        log::error!("failed to open {:?}: {}", path, err);
                        return None;
                    }
                };

                // Decompress or read into a buffer first to avoid stream bottlenecks
                let buffer = if file_name.ends_with(".gz") {
                    let mut gz = GzDecoder::new(&mut file);
                    let mut buffer = Vec::new();
                    match gz.read_to_end(&mut buffer) {
                        Ok(_) => buffer,
                        Err(err) => {
                            log::error!("failed to decompress {:?}: {}", path, err);
                            return None;
                        }
                    }
                } else {
                    let mut buffer = Vec::new();
                    match file.read_to_end(&mut buffer) {
                        Ok(_) => buffer,
                        Err(err) => {
                            log::error!("failed to read {:?}: {}", path, err);
                            return None;
                        }
                    }
                };

                if file_name.ends_with(".xml") || file_name.ends_with(".xml.gz") {
                    match self.parse_xml(path, &buffer) {
                        Ok(ok) => Some(ok),
                        Err(err) => {
                            log::warn!("skipping unreadable AppStream file {:?}: {}", path, err);
                            None
                        }
                    }
                } else if file_name.ends_with(".yml") || file_name.ends_with(".yml.gz") {
                    match self.parse_yaml(path, &buffer) {
                        Ok(ok) => Some(ok),
                        Err(err) => {
                            log::warn!("skipping unreadable AppStream file {:?}: {}", path, err);
                            None
                        }
                    }
                } else {
                    log::error!("unknown appstream file type: {:?}", path);
                    None
                }
            })
            .collect();

        for (origin_opt, infos, addons) in path_results {
            for (id, info) in infos {
                for pkgname in &info.pkgnames {
                    self.pkgnames
                        .entry(pkgname.clone())
                        .or_default()
                        .insert(id.clone());
                }
                if let Some(_old) = self.infos.insert(id.clone(), info) {
                    //TODO: merge based on priority
                    log::debug!("found duplicate info {:?}", id);
                }
            }

            for addon in addons {
                let id = AppId::new(&addon.id.0);
                for extend_id in addon.extends.iter() {
                    self.addons
                        .entry(AppId::new(&extend_id.0))
                        .or_default()
                        .push(id.clone());
                }
                let addon_info = Arc::new(AppInfo::new(
                    &self.source_id,
                    &self.source_name,
                    origin_opt.as_deref(),
                    addon,
                    &self.locale,
                    stats::try_monthly_downloads(&id).unwrap_or(0),
                    false,
                    stats::try_wayland_compatibility(&id),
                ));
                if let Some(_old) = self.infos.insert(id.clone(), addon_info) {
                    //TODO: merge based on priority
                    log::debug!("found duplicate info {:?}", id);
                }
            }
        }
    }

    fn parse_xml<P: AsRef<Path>>(
        &self,
        path: P,
        buffer: &[u8],
    ) -> Result<ParsedData, Box<dyn Error>> {
        let start = Instant::now();
        let path = path.as_ref();
        //TODO: just running this and not saving the results makes a huge memory leak!
        let e = xmltree::Element::parse(buffer)?;
        let _version = e
            .attributes
            .get("version")
            .ok_or_else(|| ParseError::missing_attribute("version", "collection"))?;
        let origin_opt = e.attributes.get("origin");
        let _arch_opt = e.attributes.get("architecture");
        let addons = Mutex::new(Vec::new());
        let infos: Vec<_> = e
            .children
            .par_iter()
            .filter_map(|node| {
                if let xmltree::XMLNode::Element(e) = node {
                    if &*e.name == "component" {
                        // Extract wayland_compat from custom fields before converting to Component
                        let wayland_compat_from_xml = extract_wayland_bitcode(e);

                        match Component::try_from(e) {
                            Ok(component) => {
                                match component.kind {
                                    ComponentKind::DesktopApplication => {}
                                    ComponentKind::Addon => {
                                        addons.lock().unwrap().push(component);
                                        return None;
                                    }
                                    _ => {
                                        // Skip anything that is not a desktop application or addon
                                        //TODO: should we allow more components?
                                        return None;
                                    }
                                }

                                let id = AppId::new(&component.id.0);
                                let monthly_downloads = 0;

                                // Use bitcode from XML if available
                                let wayland_compat = wayland_compat_from_xml;

                                if wayland_compat_from_xml.is_some() {
                                    log::debug!(
                                        "Using wayland_compat from AppStream XML for {}",
                                        component.id.0
                                    );
                                }

                                return Some((
                                    id,
                                    Arc::new(AppInfo::new(
                                        &self.source_id,
                                        &self.source_name,
                                        origin_opt.map(|x| x.as_str()),
                                        component,
                                        &self.locale,
                                        monthly_downloads,
                                        false,
                                        wayland_compat,
                                    )),
                                ));
                            }
                            Err(err) => {
                                // Unsupported component types (for example web apps) are expected.
                                log::debug!(
                                    "skipping component {:?} in {:?}: {}",
                                    e.get_child("id")
                                        .and_then(|x| appstream::AppId::try_from(x).ok()),
                                    path,
                                    err
                                );
                            }
                        }
                    }
                }
                None
            })
            .collect();
        let duration = start.elapsed();
        log::info!(
            "loaded {} items from {:?} in {:?}",
            infos.len(),
            path,
            duration
        );
        Ok((origin_opt.cloned(), infos, addons.into_inner().unwrap()))
    }

    fn parse_yaml<P: AsRef<Path>>(
        &self,
        path: P,
        buffer: &[u8],
    ) -> Result<ParsedData, Box<dyn Error>> {
        let start = Instant::now();
        let path = path.as_ref();

        // Convert buffer to string once
        let yaml_str = match std::str::from_utf8(buffer) {
            Ok(s) => s,
            Err(e) => return Err(Box::new(e)),
        };

        // Parse all documents sequentially first - this is fast for structure
        let mut documents = Vec::new();
        for (index, document) in serde_yaml::Deserializer::from_str(yaml_str).enumerate() {
            match serde_yaml::Value::deserialize(document) {
                Ok(value) => documents.push(value),
                // The header defines repository identity. Some distribution catalogs
                // (for example Zorin's extra repository) repeat a key in it, which
                // YAML parsing rejects; keep the components and lose only the origin.
                // A repeated Origin or MediaBaseUrl stays an error: it is ambiguous.
                Err(error) if index == 0 && is_harmless_duplicate_header_key(&error) => {
                    log::warn!(
                        "ignoring malformed AppStream header in {:?}: {}",
                        path,
                        error
                    );
                    documents.push(serde_yaml::Value::Null);
                }
                // Any other header error means this is not an AppStream catalog.
                Err(error) if index == 0 => return Err(Box::new(error)),
                Err(error) => log::warn!(
                    "skipping malformed AppStream document {} in {:?}: {}",
                    index,
                    path,
                    error
                ),
            }
        }

        if documents.is_empty() {
            return Ok((None, Vec::new(), Vec::new()));
        }

        // Extract header info from the first document
        let origin_opt = documents
            .first()
            .and_then(|v| v["Origin"].as_str())
            .map(|x| x.to_string());

        let media_base_url_opt = documents
            .first()
            .and_then(|v| v["MediaBaseUrl"].as_str())
            .map(|x| x.to_string());

        // Parallel process the rest
        let infos: Vec<_> = documents[1..]
            .par_iter()
            .filter_map(|value| {
                // Sanitize categories in YAML before deserialization
                let mut sanitized_value = value.clone();
                if let Some(categories) = sanitized_value.get_mut("Categories") {
                    if let Some(cat_seq) = categories.as_sequence_mut() {
                        for cat in cat_seq.iter_mut() {
                            if let Some(cat_str) = cat.as_str() {
                                let cleaned =
                                    cat_str.trim_end_matches(|c: char| c.is_ascii_punctuation());
                                *cat = serde_yaml::Value::String(cleaned.to_string());
                            }
                        }
                    }
                }

                match Component::deserialize(&sanitized_value) {
                    Ok(mut component) => {
                        if component.kind != ComponentKind::DesktopApplication {
                            // Skip anything that is not a desktop application
                            //TODO: should we allow more components?
                            return None;
                        }

                        //TODO: move to appstream crate
                        if let Some(icons) = value["Icon"].as_mapping() {
                            for (key, icon) in icons.iter() {
                                match key.as_str() {
                                    Some("cached") => match icon.as_sequence() {
                                        Some(sequence) => {
                                            for cached in sequence {
                                                match cached["name"].as_str() {
                                                    Some(name) => {
                                                        component.icons.push(Icon::Cached {
                                                            //TODO: add prefix?
                                                            path: PathBuf::from(name),
                                                            //TODO: handle parsing errors for these numbers
                                                            width: cached["width"]
                                                                .as_u64()
                                                                .and_then(|x| x.try_into().ok()),
                                                            height: cached["height"]
                                                                .as_u64()
                                                                .and_then(|x| x.try_into().ok()),
                                                            scale: cached["scale"]
                                                                .as_u64()
                                                                .and_then(|x| x.try_into().ok()),
                                                        });
                                                    }
                                                    None => {
                                                        log::warn!(
                                                            "unsupported cached icon {:?} for {:?} in {:?}",
                                                            cached,
                                                            component.id,
                                                            path
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            log::warn!(
                                                "cached icon is not a sequence in {:?}",
                                                path
                                            );
                                        }
                                    },
                                    Some("remote") => {
                                        // For now we just ignore remote icons
                                        log::debug!(
                                            "ignoring remote icons {:?} for {:?} in {:?}",
                                            icon,
                                            component.id,
                                            path
                                        );
                                    }
                                    Some("stock") => match icon.as_str() {
                                        Some(stock) => {
                                            component.icons.push(Icon::Stock(stock.to_string()));
                                        }
                                        None => {
                                            log::warn!(
                                                "unsupported stock icon {:?} for {:?} in {:?}",
                                                icon,
                                                component.id,
                                                path
                                            );
                                        }
                                    },
                                    _ => {
                                        log::warn!(
                                            "unsupported icon kind {:?} for {:?} in {:?}",
                                            key,
                                            component.id,
                                            path
                                        );
                                    }
                                }
                            }
                        }

                        if let Some(launchables) = value["Launchable"].as_mapping() {
                            for (key, launchable) in launchables.iter() {
                                match key.as_str() {
                                    Some("desktop-id") => match launchable.as_sequence() {
                                        Some(sequence) => {
                                            for desktop_id in sequence {
                                                match desktop_id.as_str() {
                                                    Some(desktop_id) => {
                                                        component.launchables.push(
                                                            Launchable::DesktopId(
                                                                desktop_id.to_string(),
                                                            ),
                                                        );
                                                    }
                                                    None => {
                                                        log::warn!(
                                                            "unsupported desktop-id launchable {:?} for {:?} in {:?}",
                                                            desktop_id,
                                                            component.id,
                                                            path
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            log::warn!(
                                                "desktop-id launchable value is not a sequence in {:?}",
                                                path
                                            );
                                        }
                                    },
                                    _ => {
                                        log::warn!(
                                            "unsupported desktop-id launchables {:?} for {:?} in {:?}",
                                            launchable,
                                            component.id,
                                            path
                                        );
                                    }
                                }
                            }
                        }

                        if let Some(provides) = value["Provides"].as_mapping() {
                            for (key, provide) in provides.iter() {
                                match key.as_str() {
                                    Some("ids") => match provide.as_sequence() {
                                        Some(sequence) => {
                                            for id in sequence {
                                                match id.as_str() {
                                                    Some(id) => {
                                                        component.provides.push(Provide::Id(
                                                            appstream::AppId(id.to_string()),
                                                        ));
                                                    }
                                                    None => {
                                                        log::warn!(
                                                            "unsupported ids provide {:?} for {:?} in {:?}",
                                                            id,
                                                            component.id,
                                                            path
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            log::warn!(
                                                "ids provide value is not a sequence in {:?}",
                                                path
                                            );
                                        }
                                    },
                                    Some("binaries") => match provide.as_sequence() {
                                        Some(sequence) => {
                                            for binary in sequence {
                                                match binary.as_str() {
                                                    Some(binary) => {
                                                        component.provides.push(Provide::Binary(
                                                            binary.to_string(),
                                                        ));
                                                    }
                                                    None => {
                                                        log::warn!(
                                                            "unsupported binary provide {:?} for {:?} in {:?}",
                                                            binary,
                                                            component.id,
                                                            path
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            log::warn!(
                                                "binaries provide value is not a sequence in {:?}",
                                                path
                                            );
                                        }
                                    },
                                    Some("mediatypes") => match provide.as_sequence() {
                                        Some(sequence) => {
                                            for mediatype in sequence {
                                                match mediatype.as_str() {
                                                    Some(mediatype) => {
                                                        component.provides.push(
                                                            Provide::MediaType(
                                                                mediatype.to_string(),
                                                            ),
                                                        );
                                                    }
                                                    None => {
                                                        log::warn!(
                                                            "unsupported mediatype provide {:?} for {:?} in {:?}",
                                                            mediatype,
                                                            component.id,
                                                            path
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            log::warn!(
                                                "mediatypes provide value is not a sequence in {:?}",
                                                path
                                            );
                                        }
                                    },
                                    // Treat mimetypes same as mediatypes (legacy key)
                                    Some("mimetypes") => match provide.as_sequence() {
                                        Some(sequence) => {
                                            for mediatype in sequence {
                                                match mediatype.as_str() {
                                                    Some(mediatype) => {
                                                        component.provides.push(
                                                            Provide::MediaType(
                                                                mediatype.to_string(),
                                                            ),
                                                        );
                                                    }
                                                    None => {
                                                        log::warn!(
                                                            "unsupported mediatype provide {:?} for {:?} in {:?}",
                                                            mediatype,
                                                            component.id,
                                                            path
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            log::warn!(
                                                "mimetypes provide value is not a sequence in {:?}",
                                                path
                                            );
                                        }
                                    },
                                    Some("python3") => {
                                        // Ignore python3 provides for now
                                        log::debug!(
                                            "ignoring {:?} provide for {:?} in {:?}",
                                            key,
                                            component.id,
                                            path
                                        );
                                    }
                                    _ => {
                                        log::warn!(
                                            "unsupported provide key {:?} value {:?} for {:?} in {:?}",
                                            key,
                                            provide,
                                            component.id,
                                            path
                                        );
                                    }
                                }
                            }
                        }

                        if let Some(releases) = value["Releases"].as_sequence() {
                            for release_value in releases {
                                if let Some(release) = release_value.as_mapping() {
                                    //TODO: read more fields
                                    let component_release = Release {
                                        date: release
                                            .get("unix-timestamp")
                                            .and_then(|x| x.as_i64())
                                            .and_then(|timestamp| {
                                                chrono::DateTime::<chrono::Utc>::from_timestamp(
                                                    timestamp, 0,
                                                )
                                            }),
                                        date_eol: None,
                                        version: release
                                            .get("version")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        description: release
                                            .get("description")
                                            .and_then(|x| x.as_mapping())
                                            .map(|x| {
                                                //TODO: more efficient way to convert this
                                                let mut items = BTreeMap::new();
                                                for (key, value) in
                                                    x.into_iter().filter_map(|(key, value)| {
                                                        Some((key.as_str()?, value.as_str()?))
                                                    })
                                                {
                                                    items
                                                        .insert(key.to_string(), value.to_string());
                                                }
                                                MarkupTranslatableString(items)
                                            }),
                                        kind: release
                                            .get("type")
                                            .and_then(|x| x.as_str())
                                            .and_then(|x| match x {
                                                "stable" => Some(ReleaseKind::Stable),
                                                "development" => Some(ReleaseKind::Development),
                                                _ => None,
                                            })
                                            .unwrap_or_default(),
                                        sizes: Vec::new(),
                                        urgency: release
                                            .get("urgency")
                                            .and_then(|x| x.as_str())
                                            .and_then(|x| match x {
                                                "low" => Some(ReleaseUrgency::Low),
                                                "medium" => Some(ReleaseUrgency::Medium),
                                                "high" => Some(ReleaseUrgency::High),
                                                "critical" => Some(ReleaseUrgency::Critical),
                                                _ => None,
                                            })
                                            .unwrap_or_default(),
                                        artifacts: Vec::new(),
                                        url: None,
                                    };
                                    component.releases.push(component_release)
                                }
                            }
                        }

                        if let Some(screenshots) = value["Screenshots"].as_sequence() {
                            for screenshot_value in screenshots {
                                if let Some(screenshot) = screenshot_value.as_mapping() {
                                    let mut images = Vec::new();
                                    if let Some(source_image) =
                                        screenshot.get("source-image").and_then(|x| x.as_mapping())
                                    {
                                        if let Some(path_str) = source_image["url"].as_str() {
                                            let url_str = match &media_base_url_opt {
                                                Some(media_base_url) => {
                                                    //TODO: join using url crate?
                                                    format!("{media_base_url}/{path_str}")
                                                }
                                                None => path_str.to_string(),
                                            };
                                            match Url::parse(&url_str) {
                                                Ok(url) => {
                                                    images.push(Image {
                                                        kind: ImageKind::Source,
                                                        width: None,
                                                        height: None,
                                                        url,
                                                    });
                                                }
                                                Err(err) => {
                                                    log::warn!(
                                                        "failed to parse {:?}: {}",
                                                        url_str,
                                                        err
                                                    );
                                                }
                                            }
                                        }
                                    }

                                    //TODO: thumbnails

                                    component.screenshots.push(Screenshot {
                                        //TODO: set is_default
                                        is_default: false,
                                        //TODO: caption
                                        caption: None,
                                        images,
                                        //TODO: videos?
                                        videos: Vec::new(),
                                    });
                                }
                            }
                        }

                        if let Some(urls) = value["Url"].as_mapping() {
                            for (key, url_value) in urls.iter() {
                                let url = match url_value.as_str() {
                                    Some(url_str) => match Url::parse(url_str) {
                                        Ok(ok) => ok,
                                        Err(err) => {
                                            log::warn!(
                                                "failed to parse {:?} for {:?} in {:?}: {}",
                                                url_str,
                                                component.id,
                                                path,
                                                err
                                            );
                                            continue;
                                        }
                                    },
                                    None => {
                                        log::debug!(
                                            "unsupported url kind {:?} for {:?} in {:?}",
                                            url_value,
                                            component.id,
                                            path
                                        );
                                        continue;
                                    }
                                };

                                let project_url = match key.as_str() {
                                    Some("bugtracker") => ProjectUrl::BugTracker(url),
                                    Some("contact") => ProjectUrl::Contact(url),
                                    //TODO: add to appstream crate: Some("contribute") => ProjectUrl::Contribute(url),
                                    Some("donation") => ProjectUrl::Donation(url),
                                    Some("faq") => ProjectUrl::Faq(url),
                                    Some("help") => ProjectUrl::Help(url),
                                    Some("homepage") => ProjectUrl::Homepage(url),
                                    Some("translate") => ProjectUrl::Translate(url),
                                    //TODO: add to appstream crate: Some("vcs-browser") => ProjectUrl::VcsBrowser(url),
                                    _ => {
                                        log::debug!(
                                            "unsupported url kind {:?} for {:?} in {:?}",
                                            key,
                                            component.id,
                                            path
                                        );
                                        continue;
                                    }
                                };

                                component.urls.push(project_url);
                            }
                        }

                        let id = AppId::new(&component.id.0);
                        let monthly_downloads = 0;

                        // Extract wayland_compat from YAML custom fields if available
                        let wayland_compat_from_yaml = value
                            .get("Custom")
                            .and_then(|custom| custom.get("wayland_compat"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.strip_prefix("0x"))
                            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                            .map(|bitcode| {
                                log::debug!(
                                    "Found wayland_compat bitcode in YAML: 0x{:02X}",
                                    bitcode
                                );
                                WaylandCompatibility::decode_bitcode(bitcode)
                            });

                        // Use bitcode from YAML if available, otherwise fall back to stats
                        let wayland_compat = wayland_compat_from_yaml;

                        if wayland_compat_from_yaml.is_some() {
                            log::debug!(
                                "Using wayland_compat from AppStream YAML for {}",
                                component.id.0
                            );
                        }

                        Some((
                            id,
                            Arc::new(AppInfo::new(
                                &self.source_id,
                                &self.source_name,
                                origin_opt.as_deref(),
                                component,
                                &self.locale,
                                monthly_downloads,
                                false,
                                wayland_compat,
                            )),
                        ))
                    }
                    Err(err) => {
                        log::debug!("skipping component {:?} in {:?}: {}", value["ID"], path, err);
                        None
                    }
                }
            })
            .collect();
        let duration = start.elapsed();
        log::info!(
            "loaded {} items from {:?} in {:?}",
            infos.len(),
            path,
            duration
        );
        Ok((origin_opt, infos, Vec::new()))
    }
}

#[cfg(test)]
mod wayland_bitcode_tests {
    use super::*;
    use crate::app_info::{AppFramework, RiskLevel, WaylandSupport};

    #[test]
    fn test_extract_wayland_bitcode() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<component>
  <id>org.gnome.Epiphany</id>
  <name>Web</name>
  <custom>
    <value key="wayland_compat">0x0A</value>
  </custom>
</component>
"##;

        let element = xmltree::Element::parse(xml.as_bytes()).unwrap();
        let wayland_compat = extract_wayland_bitcode(&element);

        assert!(wayland_compat.is_some());
        let compat = wayland_compat.unwrap();
        assert_eq!(compat.framework, AppFramework::GTK4);
        assert_eq!(compat.support, WaylandSupport::Native);
        assert_eq!(compat.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_extract_wayland_bitcode_multiple_custom_values() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<component>
  <id>com.brave.Browser</id>
  <name>Brave</name>
  <custom>
    <value key="other_key">some_value</value>
    <value key="wayland_compat">0x96</value>
  </custom>
</component>
"##;

        let element = xmltree::Element::parse(xml.as_bytes()).unwrap();
        let wayland_compat = extract_wayland_bitcode(&element);

        assert!(wayland_compat.is_some());
        let compat = wayland_compat.unwrap();
        assert_eq!(compat.framework, AppFramework::Electron);
        assert_eq!(compat.support, WaylandSupport::Native);
        assert_eq!(compat.risk_level, RiskLevel::High);
    }

    #[test]
    fn test_extract_wayland_bitcode_no_custom() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<component>
  <id>org.example.App</id>
  <name>Example</name>
</component>
"##;

        let element = xmltree::Element::parse(xml.as_bytes()).unwrap();
        let wayland_compat = extract_wayland_bitcode(&element);

        assert!(wayland_compat.is_none());
    }

    #[test]
    fn test_extract_wayland_bitcode_qt6() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<component>
  <id>org.kde.kate</id>
  <name>Kate</name>
  <custom>
    <value key="wayland_compat">0x52</value>
  </custom>
</component>
"##;

        let element = xmltree::Element::parse(xml.as_bytes()).unwrap();
        let wayland_compat = extract_wayland_bitcode(&element);

        assert!(wayland_compat.is_some());
        let compat = wayland_compat.unwrap();
        assert_eq!(compat.framework, AppFramework::Qt6);
        assert_eq!(compat.support, WaylandSupport::Native);
        assert_eq!(compat.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn test_parse_xml_with_wayland_bitcode() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<components version="0.16">
  <component type="desktop-application">
    <id>org.gnome.Epiphany</id>
    <name>Web</name>
    <summary>Web browser for GNOME</summary>
    <custom>
      <value key="wayland_compat">0x0A</value>
    </custom>
  </component>
  <component type="desktop-application">
    <id>com.brave.Browser</id>
    <name>Brave</name>
    <summary>Web browser</summary>
    <custom>
      <value key="wayland_compat">0x96</value>
    </custom>
  </component>
</components>
"##;

        let cache = AppstreamCache {
            source_id: "test".to_string(),
            source_name: "Test".to_string(),
            ..Default::default()
        };

        let result = cache.parse_xml("test.xml", xml.as_bytes());
        assert!(result.is_ok());

        let (_origin, infos, _addons) = result.unwrap();
        assert_eq!(infos.len(), 2);

        // Check first app (Epiphany - GTK4 + Native + Low)
        let (_id, info1) = &infos[0];
        assert_eq!(info1.name, "Web");
        assert!(info1.wayland_compat.is_some());
        let compat1 = info1.wayland_compat.unwrap();
        assert_eq!(compat1.framework, AppFramework::GTK4);
        assert_eq!(compat1.support, WaylandSupport::Native);
        assert_eq!(compat1.risk_level, RiskLevel::Low);

        // Check second app (Brave - Electron + Native + High)
        let (_id, info2) = &infos[1];
        assert_eq!(info2.name, "Brave");
        assert!(info2.wayland_compat.is_some());
        let compat2 = info2.wayland_compat.unwrap();
        assert_eq!(compat2.framework, AppFramework::Electron);
        assert_eq!(compat2.support, WaylandSupport::Native);
        assert_eq!(compat2.risk_level, RiskLevel::High);
    }

    #[test]
    fn test_parse_mock_appstream_file() {
        use std::fs::File;
        use std::io::BufReader;

        // Load mock AppStream file
        let file = File::open("res/mock-appstream-wayland.xml");
        if file.is_err() {
            // Skip test if file doesn't exist (e.g., in CI)
            eprintln!("Skipping test: res/mock-appstream-wayland.xml not found");
            return;
        }

        let file = file.unwrap();
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).unwrap();

        let cache = AppstreamCache {
            source_id: "test".to_string(),
            source_name: "Test".to_string(),
            ..Default::default()
        };

        let result = cache.parse_xml("mock-appstream-wayland.xml", &buffer);
        assert!(result.is_ok(), "Failed to parse mock AppStream file");

        let (_origin, infos, _addons) = result.unwrap();
        assert_eq!(infos.len(), 12, "Expected 12 apps in mock file");

        // Verify specific apps have correct bitcode data
        for (id, info) in &infos {
            let id_str = id.raw();
            println!("Testing app: {} ({})", info.name, id_str);
            assert!(
                info.wayland_compat.is_some(),
                "App {} should have wayland_compat",
                id_str
            );

            let compat = info.wayland_compat.unwrap();

            match id_str {
                "org.gnome.Epiphany" | "org.gnome.Nautilus" | "org.gnome.TextEditor" => {
                    // GTK4 + Native + Low (0x0A)
                    assert_eq!(
                        compat.framework,
                        AppFramework::GTK4,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::Low,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                "org.kde.kate" | "org.kde.okular" | "org.kde.krita" => {
                    // Qt6 + Native + Medium (0x52)
                    assert_eq!(
                        compat.framework,
                        AppFramework::Qt6,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::Medium,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                "com.brave.Browser" => {
                    // Electron + Native + High (0x96)
                    assert_eq!(
                        compat.framework,
                        AppFramework::Electron,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::High,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                "org.gimp.GIMP" | "org.inkscape.Inkscape" => {
                    // GTK3 + Native + Low (0x06)
                    assert_eq!(
                        compat.framework,
                        AppFramework::GTK3,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::Low,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                "org.videolan.VLC" => {
                    // Qt5 + Native + Medium (0x4E)
                    assert_eq!(
                        compat.framework,
                        AppFramework::Qt5,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::Medium,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                "com.visualstudio.code" => {
                    // Electron + Native + High
                    assert_eq!(
                        compat.framework,
                        AppFramework::Electron,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::High,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                "com.discordapp.Discord" => {
                    // Electron + Native + High (Mock data values)
                    assert_eq!(
                        compat.framework,
                        AppFramework::Electron,
                        "Wrong framework for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.support,
                        WaylandSupport::Native,
                        "Wrong support for {}",
                        id_str
                    );
                    assert_eq!(
                        compat.risk_level,
                        RiskLevel::High,
                        "Wrong risk level for {}",
                        id_str
                    );
                }
                _ => panic!("Unexpected app ID: {}", id_str),
            }
        }

        println!("✅ All 12 apps parsed correctly with bitcode data!");
    }
}
