mod data;
mod handlers;
mod views;

use cosmic::{
    Application, ApplicationExt, Element, action,
    app::{Core, Task, context_drawer},
    cosmic_config::{self},
    cosmic_theme, executor,
    iced::{
        Alignment, Length, Size, Subscription,
        widget::scrollable,
        window::{self},
    },
    theme,
    widget::{self},
};
use rayon::prelude::*;
use std::{
    cell::Cell,
    cmp,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::app_entry::{AppEntry, Apps};
use crate::app_id::AppId;
use crate::app_info::{AppInfo, AppProvide};
use crate::backend::{self, Backends, Package};
use crate::category::Category;
use crate::cli::Flags;
use crate::config::{AppTheme, Config};
use crate::constants::MAX_GRID_WIDTH;
use crate::gstreamer::Mode;

use crate::key_bind::{KeyBind, key_binds};
use crate::localize::LANGUAGE_SORTER;
use crate::pages::{ContextPage, DialogPage, ExplorePage, NavPage};
use crate::pages::{DetailsPage, DetailsPageActions, SelectedSource};
use crate::search::{SearchResult, SearchSortMode, WaylandFilter};
use crate::ui::{GridMetrics, package_card_view};

use crate::fl;

use crate::message::{Action, Message};
use crate::operation::{Operation, OperationKind};
use crate::os_info::OsInfo;
use crate::priority::priority;
use crate::scroll_context::ScrollContext;
use crate::source::{Source, SourceKind};

// impl Package is here.

impl Package {
    pub fn grid_metrics(spacing: &cosmic_theme::Spacing, width: usize) -> GridMetrics {
        GridMetrics::new(width, 320 + 2 * spacing.space_s as usize, spacing.space_xxs)
    }

    pub fn card_view<'a>(
        &'a self,
        controls: Vec<Element<'a, Message>>,
        top_controls: Option<Vec<Element<'a, Message>>>,
        spacing: &cosmic_theme::Spacing,
        width: usize,
        app_stats: &'a std::collections::HashMap<
            crate::app_id::AppId,
            (u64, Option<crate::app_info::WaylandCompatibility>),
        >,
    ) -> Element<'a, Message> {
        package_card_view(
            &self.info,
            Some(&self.icon),
            controls,
            top_controls,
            spacing,
            width,
            app_stats,
        )
    }
}

pub struct App {
    pub(crate) core: Core,
    pub(crate) config_handler: Option<cosmic_config::Config>,
    pub(crate) config: Config,
    pub(crate) mode: Mode,
    pub(crate) locale: String,
    pub(crate) os_codename: String,
    pub(crate) app_themes: Vec<String>,
    pub(crate) apps: Arc<Apps>,
    pub(crate) catalog_apps: Apps,
    pub(crate) backends: Backends,
    pub(crate) context_page: ContextPage,
    pub(crate) dialog_pages: VecDeque<DialogPage>,
    pub(crate) explore_page_opt: Option<ExplorePage>,
    pub(crate) key_binds: HashMap<KeyBind, Action>,
    pub(crate) nav_model: widget::nav_bar::Model,
    pub(crate) nav_ids: Vec<(NavPage, widget::nav_bar::Id)>,
    pub(crate) nav_history: Vec<(NavPage, usize)>,
    pub(crate) subcategory: usize,
    pub(crate) subcategory_options: Vec<String>,
    #[cfg(feature = "notify")]
    pub(crate) notification_opt: Option<Arc<Mutex<notify_rust::NotificationHandle>>>,
    pub(crate) pending_operation_id: u64,
    pub(crate) pending_operations: BTreeMap<u64, (Operation, f32)>,
    pub(crate) progress_operations: BTreeSet<u64>,
    pub(crate) complete_operations: BTreeMap<u64, Operation>,
    pub(crate) failed_operations: BTreeMap<u64, (Operation, f32, String)>,
    pub(crate) repos_changing: Vec<(&'static str, String, bool)>,
    pub(crate) scrollable_id: widget::Id,
    pub(crate) scroll_views: HashMap<ScrollContext, scrollable::Viewport>,
    pub(crate) search_active: bool,
    pub(crate) search_id: widget::Id,
    pub(crate) search_input: String,
    pub(crate) search_sort_mode: SearchSortMode,
    pub(crate) store_source: crate::search::StoreSource,
    pub(crate) store_source_options: Vec<String>,
    pub(crate) native_only: bool,
    pub(crate) results_limit: usize,
    pub(crate) search_pending: bool,
    pub(crate) explore_loading: bool,
    pub(crate) search_sort_options: Vec<String>,
    pub(crate) wayland_filter: WaylandFilter,
    pub(crate) wayland_filter_options: Vec<String>,
    pub(crate) size: Cell<Option<Size>>,
    //TODO: use hashset?
    pub(crate) installed: Option<Vec<(&'static str, Package)>>,
    //TODO: use hashset?
    pub(crate) updates: Option<Vec<(&'static str, Package)>>,
    //TODO: use hashset?
    pub(crate) waiting_installed: Vec<(&'static str, String, AppId)>,
    //TODO: use hashset?
    pub(crate) waiting_updates: Vec<(&'static str, String, AppId)>,
    pub(crate) category_results: Option<(&'static [Category], Vec<SearchResult>)>,
    pub(crate) explore_results: HashMap<ExplorePage, Vec<SearchResult>>,
    pub(crate) installed_results: Option<Vec<SearchResult>>,
    pub(crate) search_results: Option<(String, Vec<SearchResult>)>,
    pub(crate) details_page_opt: Option<DetailsPage>,
    pub(crate) applet_placement_buttons: cosmic::widget::segmented_button::SingleSelectModel,
    pub(crate) uninstall_purge_data: bool,
    pub(crate) loading_frame: usize,
    pub(crate) app_stats: HashMap<AppId, (u64, Option<crate::app_info::WaylandCompatibility>)>,
}

impl DetailsPageActions for App {
    fn selected_buttons<'a>(
        &'a self,
        backend_name: &'static str,
        id: &AppId,
        info: &Arc<AppInfo>,
        addon: bool,
    ) -> Vec<Element<'a, Message>> {
        self.selected_buttons_impl(backend_name, id, info, addon)
    }
}

impl App {
    pub(crate) fn open_desktop_id(&self, mut desktop_id: String) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if !desktop_id.ends_with(".desktop") {
                        desktop_id.push_str(".desktop");
                    }
                    let xdg_dirs = xdg::BaseDirectories::with_prefix("applications");
                    let path = match xdg_dirs.find_data_file(&desktop_id) {
                        Some(some) => some,
                        None => {
                            log::warn!("failed to find desktop file for {:?}", desktop_id);
                            return None;
                        }
                    };
                    let entry = match freedesktop_entry_parser::parse_entry(&path) {
                        Ok(ok) => ok,
                        Err(err) => {
                            log::warn!("failed to read desktop file {:?}: {}", path, err);
                            return None;
                        }
                    };
                    //TODO: handle Terminal=true
                    let Some(exec) = entry
                        .get("Desktop Entry", "Exec")
                        .and_then(|attr| attr.first())
                    else {
                        log::warn!("no exec section in {:?}", path);
                        return None;
                    };
                    //TODO: use libcosmic for loading desktop data
                    Some((exec.to_string(), desktop_id))
                })
                .await
                .unwrap_or(None)
            },
            |result| {
                #[cfg(feature = "desktop")]
                if let Some((exec, desktop_id)) = result {
                    tokio::spawn(async move {
                        cosmic::desktop::spawn_desktop_exec(
                            &exec,
                            Vec::<(&str, &str)>::new(),
                            Some(&desktop_id),
                            false,
                        )
                        .await;
                    });
                }
                action::none()
            },
        )
    }

    pub(crate) fn operation(&mut self, operation: Operation) {
        match &operation.kind {
            OperationKind::RepositoryAdd(adds) => {
                for add in adds.iter() {
                    self.repos_changing
                        .push((operation.backend_name, add.id.clone(), true));
                }
            }
            OperationKind::RepositoryRemove(rms, _) => {
                for rm in rms.iter() {
                    self.repos_changing
                        .push((operation.backend_name, rm.id.clone(), false));
                }
            }
            _ => {}
        }

        let id = self.pending_operation_id;
        self.pending_operation_id += 1;
        self.progress_operations.insert(id);
        self.pending_operations.insert(id, (operation, 0.0));
    }

    pub(crate) fn restore_game_sort(&mut self) {
        if self.nav_model.active_data::<NavPage>() == Some(&NavPage::Game)
            && self.search_sort_mode == SearchSortMode::Relevance
        {
            self.search_sort_mode = SearchSortMode::NewestGames;
            if let Some((_, results)) = &mut self.category_results {
                crate::search_logic::sort_results(results, self.search_sort_mode, &self.app_stats);
            }
        }
    }

    pub(crate) fn active_categories(&self) -> Option<&'static [Category]> {
        let page = self.nav_model.active_data::<NavPage>()?;
        if self.subcategory > 0 {
            page.subcategories()
                .get(self.subcategory - 1)
                .map(|subcategory| subcategory.categories)
        } else {
            page.categories()
        }
    }

    pub(crate) fn can_go_back(&self) -> bool {
        self.details_page_opt.is_some()
            || self.search_active
            || self.explore_page_opt.is_some()
            || self.subcategory > 0
            || !self.nav_history.is_empty()
    }

    pub(crate) fn go_back(&mut self) -> Task<Message> {
        if self.details_page_opt.take().is_some() {
            log::info!("back to catalog from details");
            return self.update_scroll();
        }
        if self.search_active || self.search_results.is_some() {
            return self.update(Message::SearchClear);
        }
        if self.explore_page_opt.take().is_some() {
            return self.update_scroll();
        }
        if self.subcategory > 0 {
            return self.update(Message::Subcategory(0));
        }
        if let Some((page, subcategory)) = self.nav_history.pop() {
            if let Some(id) = self
                .nav_ids
                .iter()
                .find(|(p, _)| *p == page)
                .map(|(_, id)| *id)
            {
                let task = self.on_nav_select(id);
                self.nav_history.pop(); // Back must not add a forward entry.
                if subcategory > 0 {
                    return Task::batch(vec![task, self.update(Message::Subcategory(subcategory))]);
                }
                return task;
            }
        }
        Task::none()
    }

    pub(crate) fn categories(&self, categories: &'static [Category]) -> Task<Message> {
        data::categories_task(
            self.apps.clone(),
            self.backends.clone(),
            self.app_stats.clone(),
            self.os_codename.clone(),
            categories,
        )
    }

    #[allow(dead_code)]
    fn explore_results(&self, explore_page: ExplorePage) -> Task<Message> {
        data::explore_results_task(
            self.apps.clone(),
            self.backends.clone(),
            self.app_stats.clone(),
            self.os_codename.clone(),
            explore_page,
        )
    }

    pub(crate) fn explore_results_all_batch(&mut self) -> Task<Message> {
        self.explore_loading = true;
        data::explore_results_all_batch_task(
            self.apps.clone(),
            self.backends.clone(),
            self.app_stats.clone(),
            self.os_codename.clone(),
        )
    }

    pub(crate) fn installed_results(&self) -> Task<Message> {
        data::installed_results_task(
            self.apps.clone(),
            self.backends.clone(),
            self.app_stats.clone(),
            self.os_codename.clone(),
        )
    }

    pub(crate) fn load_icons_for_results(&self, results: &mut [crate::search::SearchResult]) {
        use crate::constants::MAX_RESULTS;

        // Load icons for the first page of results
        // Note: Sequential iteration because Handle is not thread-safe
        for result in results.iter_mut().take(MAX_RESULTS) {
            // Skip if icon is already loaded
            if result.icon_opt.is_some() {
                continue;
            }

            let Some(backend) = self.backends.get(result.backend_name()) else {
                continue;
            };
            let appstream_caches = backend.info_caches();
            let Some(appstream_cache) = appstream_caches
                .iter()
                .find(|x| x.source_id == result.info.source_id)
            else {
                continue;
            };
            result.icon_opt = Some(appstream_cache.icon(&result.info));
        }
    }

    pub(crate) fn filter_store_results(&self, results: &mut Vec<SearchResult>) {
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        results.retain_mut(|result| {
            let matches = |backend: &str, info: &AppInfo, installed: bool| {
                self.store_source.matches(backend)
                    && (!self.native_only || crate::search::native_linux(backend, info))
                    && (installed
                        || backend != "packagekit"
                        || self
                            .backends
                            .get(backend)
                            .is_some_and(|b| b.is_package_available(&info.pkgnames)))
            };
            if let Some(entries) = self.apps.get(&result.id) {
                if let Some(entry) = entries
                    .iter()
                    .find(|e| matches(e.backend_name, &e.info, e.installed))
                {
                    if entry.backend_name != result.backend_name()
                        || entry.info.source_id != result.info.source_id
                    {
                        *result = SearchResult::new(
                            entry.backend_name,
                            result.id.clone(),
                            None,
                            entry.info.clone(),
                            result.weight,
                        );
                    }
                    return true;
                }
                false
            } else {
                matches(result.backend_name(), &result.info, false)
            }
        });
        crate::search_logic::sort_results(results, self.search_sort_mode, &self.app_stats);
    }

    pub(crate) fn refresh_store(&mut self) -> Task<Message> {
        let mut tasks = vec![self.explore_results_all_batch()];
        if let Some(categories) = self.active_categories() {
            tasks.push(self.categories(categories));
        }
        if !self.search_input.is_empty() {
            tasks.push(self.search());
        }
        Task::batch(tasks)
    }

    pub(crate) fn search(&self) -> Task<Message> {
        data::search_task(
            self.apps.clone(),
            self.backends.clone(),
            self.app_stats.clone(),
            self.os_codename.clone(),
            self.search_input.clone(),
            self.search_sort_mode,
            self.wayland_filter,
            matches!(
                self.store_source,
                crate::search::StoreSource::All | crate::search::StoreSource::Steam
            ),
        )
    }

    fn selected_buttons_impl(
        &self,
        selected_backend_name: &'static str,
        selected_id: &AppId,
        selected_info: &Arc<AppInfo>,
        addon: bool,
    ) -> Vec<Element<'_, Message>> {
        if selected_backend_name == crate::catalog::STEAM {
            if let Some(id) = crate::catalog::steam_id(selected_info) {
                let installed = self.installed.as_ref().is_some_and(|packages| {
                    packages.iter().any(|(_, package)| {
                        package
                            .info
                            .pkgnames
                            .iter()
                            .any(|name| name == "steam" || name == "steam-installer")
                            || package.info.desktop_ids.iter().any(|name| {
                                matches!(
                                    name.as_str(),
                                    "steam.desktop" | "com.valvesoftware.Steam.desktop"
                                )
                            })
                            || package.id.normalized() == "com.valvesoftware.Steam"
                    })
                }) || crate::catalog::steam_desktop_available();
                let action = if installed {
                    Message::LaunchUrl(crate::catalog::install_url(id))
                } else {
                    Message::FindApp("Steam".into())
                };
                return vec![
                    widget::column::with_children(vec![
                        widget::text::caption(if installed {
                            fl!("steam-ready")
                        } else {
                            fl!("steam-required")
                        })
                        .into(),
                        widget::button::suggested(if installed {
                            fl!("steam-install")
                        } else {
                            fl!("steam-get-client")
                        })
                        .on_press(action)
                        .into(),
                        widget::button::standard(fl!("steam-store"))
                            .on_press(Message::LaunchUrl(crate::catalog::store_url(id)))
                            .into(),
                        widget::button::standard(fl!("steam-compatibility"))
                            .on_press(Message::LaunchUrl(format!(
                                "https://www.protondb.com/app/{id}"
                            )))
                            .into(),
                    ])
                    .spacing(8)
                    .into(),
                ];
            }
        }
        //TODO: more efficient checks
        let mut waiting_refresh = false;
        for (backend_name, source_id, package_id) in self
            .waiting_installed
            .iter()
            .chain(self.waiting_updates.iter())
        {
            if backend_name == &selected_backend_name
                && source_id == &selected_info.source_id
                && package_id == selected_id
            {
                waiting_refresh = true;
                break;
            }
        }
        let is_installed = self.is_installed(selected_backend_name, selected_id, selected_info);
        let applet_provide = AppProvide::Id("com.system76.CosmicApplet".to_string());
        let mut update_opt = None;
        if let Some(updates) = &self.updates {
            for (backend_name, package) in updates {
                if backend_name == &selected_backend_name
                    && package.info.source_id == selected_info.source_id
                    && &package.id == selected_id
                {
                    update_opt = Some(Message::Operation(
                        OperationKind::Update,
                        backend_name,
                        package.id.clone(),
                        package.info.clone(),
                    ));
                    break;
                }
            }
        }
        let mut progress_opt = None;
        for (op, progress) in self.pending_operations.values() {
            if op.backend_name == selected_backend_name
                && op
                    .infos
                    .iter()
                    .any(|info| info.source_id == selected_info.source_id)
                && op
                    .package_ids
                    .iter()
                    .any(|package_id| package_id == selected_id)
            {
                progress_opt = Some(*progress);
                break;
            }
        }

        let mut buttons = Vec::with_capacity(2);
        if let Some(progress) = progress_opt {
            //TODO: get height from theme?
            buttons.push(
                widget::progress_bar(0.0..=100.0, progress)
                    .height(Length::Fixed(4.0))
                    .into(),
            )
        } else if waiting_refresh {
            // Do not show buttons while waiting for refresh
        } else if is_installed {
            //TODO: what if there are multiple desktop IDs?
            if let Some(desktop_id) = selected_info.desktop_ids.first() {
                if selected_info.provides.contains(&applet_provide) {
                    buttons.push(
                        widget::button::suggested(fl!("place-on-desktop"))
                            .on_press(Message::DialogPage(DialogPage::Place(selected_id.clone())))
                            .into(),
                    );
                } else {
                    buttons.push(
                        widget::button::suggested(fl!("open"))
                            .on_press(Message::OpenDesktopId(desktop_id.clone()))
                            .into(),
                    );
                }
            }
            if let Some(update) = update_opt {
                buttons.push(
                    widget::button::standard(fl!("update"))
                        .on_press(update)
                        .into(),
                );
            }
            if !selected_id.is_system() {
                buttons.push(
                    widget::button::standard(fl!("uninstall"))
                        .on_press(Message::DialogPage(DialogPage::Uninstall(
                            selected_backend_name,
                            selected_id.clone(),
                            selected_info.clone(),
                        )))
                        .into(),
                );
            }
        } else {
            buttons.push(
                if addon {
                    widget::button::standard(fl!("install"))
                } else {
                    widget::button::suggested(fl!("install"))
                }
                .on_press(Message::Operation(
                    OperationKind::Install,
                    selected_backend_name,
                    selected_id.clone(),
                    selected_info.clone(),
                ))
                .into(),
            )
        }

        buttons
    }

    fn selected_sources(
        &self,
        backend_name: &'static str,
        id: &AppId,
        info: &AppInfo,
    ) -> Vec<SelectedSource> {
        let mut sources = Vec::new();
        match self.apps.get(id) {
            Some(infos) => {
                for AppEntry {
                    backend_name,
                    info,
                    installed,
                } in infos.iter()
                {
                    if *installed
                        || *backend_name != "packagekit"
                        || self
                            .backends
                            .get(backend_name)
                            .is_some_and(|backend| backend.is_package_available(&info.pkgnames))
                    {
                        sources.push(SelectedSource::new(backend_name, info, *installed));
                    }
                }
            }
            None => {
                //TODO: warning?
                let installed = self.is_installed(backend_name, id, info);
                sources.push(SelectedSource::new(backend_name, info, installed));
            }
        }
        sources
    }

    fn selected_addons(
        &self,
        backend_name: &'static str,
        id: &AppId,
        info: &AppInfo,
    ) -> Vec<(AppId, Arc<AppInfo>)> {
        let mut addons = Vec::new();
        if let Some(backend) = self.backends.get(backend_name) {
            for appstream_cache in backend.info_caches() {
                if appstream_cache.source_id == info.source_id {
                    if let Some(ids) = appstream_cache.addons.get(id) {
                        for id in ids {
                            if let Some(info) = appstream_cache.infos.get(id) {
                                addons.push((id.clone(), info.clone()));
                            }
                        }
                    }
                }
            }
        }
        addons.par_sort_unstable_by(|a, b| {
            match b.1.monthly_downloads.cmp(&a.1.monthly_downloads) {
                cmp::Ordering::Equal => LANGUAGE_SORTER.compare(&a.1.name, &b.1.name),
                ordering => ordering,
            }
        });
        addons
    }

    fn select(
        &mut self,
        backend_name: &'static str,
        id: AppId,
        icon_opt: Option<widget::icon::Handle>,
        info: Arc<AppInfo>,
    ) -> Task<Message> {
        log::info!(
            "selected {:?} from backend {:?} and source {:?}",
            id,
            backend_name,
            info.source_id
        );
        let sources = self.selected_sources(backend_name, &id, &info);
        let addons = self.selected_addons(backend_name, &id, &info);
        self.details_page_opt = Some(DetailsPage::new(
            backend_name,
            id,
            icon_opt,
            info,
            sources,
            addons,
        ));
        self.update_scroll()
    }

    pub(crate) fn scroll_context(&self) -> ScrollContext {
        if self.details_page_opt.is_some() {
            ScrollContext::DetailsPage
        } else if self.search_results.is_some() {
            ScrollContext::SearchResults
        } else if self.explore_page_opt.is_some() {
            ScrollContext::ExplorePage
        } else {
            ScrollContext::NavPage
        }
    }

    pub(crate) fn update_scroll(&mut self) -> Task<Message> {
        let scroll_context = self.scroll_context();
        // Clear unused scroll contexts
        for remove_context in scroll_context.unused_contexts() {
            self.scroll_views.remove(remove_context);
        }
        scrollable::scroll_to(
            self.scrollable_id.clone(),
            match self.scroll_views.get(&scroll_context) {
                Some(viewport) => viewport.absolute_offset(),
                None => scrollable::AbsoluteOffset::default(),
            },
        )
    }

    fn update_backends(&mut self, refresh: bool) -> Task<Message> {
        let locale = self.locale.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let start = Instant::now();
                    let backends = backend::backends(&locale, refresh);
                    let duration = start.elapsed();
                    log::info!(
                        "loaded backends {} in {:?}",
                        if refresh {
                            "with refreshing"
                        } else {
                            "without refreshing"
                        },
                        duration
                    );
                    action::app(Message::Backends(backends))
                })
                .await
                .unwrap_or(action::none())
            },
            |x| x,
        )
    }

    fn update_config(&mut self) -> Task<Message> {
        cosmic::command::set_theme(self.config.app_theme.theme())
    }

    pub(crate) fn handle_config_message(&mut self, message: Message) -> Task<Message> {
        handlers::handle_config_message(self, message)
    }

    pub(crate) fn handle_search_message(&mut self, message: Message) -> Task<Message> {
        handlers::handle_search_message(self, message)
    }

    pub(crate) fn handle_backend_message(&mut self, message: Message) -> Task<Message> {
        handlers::handle_backend_message(self, message)
    }

    pub(crate) fn handle_dialog_message(&mut self, message: Message) -> Task<Message> {
        handlers::handle_dialog_message(self, message)
    }

    pub(crate) fn handle_operation_message(&mut self, message: Message) -> Task<Message> {
        handlers::handle_operation_message(self, message)
    }

    pub(crate) fn handle_selection_message(&mut self, message: Message) -> Task<Message> {
        handlers::handle_selection_message(self, message)
    }

    fn is_installed_inner(
        installed_opt: &Option<Vec<(&'static str, Package)>>,
        backend_name: &'static str,
        id: &AppId,
        info: &AppInfo,
    ) -> bool {
        if let Some(installed) = installed_opt {
            for (installed_backend_name, package) in installed {
                if *installed_backend_name == backend_name
                    && package.info.source_id == info.source_id
                {
                    // Simple app match found
                    if &package.id == id {
                        return true;
                    }

                    // Search for matching pkgnames
                    //TODO: also do flatpak refs?
                    if package.id.is_system() && !info.pkgnames.is_empty() {
                        let mut found = true;
                        for pkgname in info.pkgnames.iter() {
                            if !package.info.pkgnames.contains(pkgname) {
                                found = false;
                                break;
                            }
                        }
                        if found {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn is_installed(&self, backend_name: &'static str, id: &AppId, info: &AppInfo) -> bool {
        Self::is_installed_inner(&self.installed, backend_name, id, info)
    }

    //TODO: run in background
    pub(crate) fn update_apps(&mut self) {
        let start = Instant::now();
        let mut apps = Apps::new();

        let entry_sort =
            |a: &AppEntry, b: &AppEntry, id: &AppId| match b.installed.cmp(&a.installed) {
                cmp::Ordering::Equal => {
                    let a_priority = priority(a.backend_name, &a.info.source_id, id);
                    let b_priority = priority(b.backend_name, &b.info.source_id, id);
                    match b_priority.cmp(&a_priority) {
                        cmp::Ordering::Equal => {
                            match LANGUAGE_SORTER.compare(&a.info.source_id, &b.info.source_id) {
                                cmp::Ordering::Equal => {
                                    LANGUAGE_SORTER.compare(a.backend_name, b.backend_name)
                                }
                                ordering => ordering,
                            }
                        }
                        ordering => ordering,
                    }
                }
                ordering => ordering,
            };

        //TODO: par_iter?
        let mapping_start = Instant::now();
        for (backend_name, backend) in self.backends.iter() {
            for appstream_cache in backend.info_caches() {
                for (id, info) in appstream_cache.infos.iter() {
                    let entry = apps.entry(id.clone()).or_default();
                    entry.push(AppEntry {
                        backend_name,
                        info: info.clone(),
                        installed: self.is_installed(backend_name, id, info),
                    });
                    entry.par_sort_unstable_by(|a, b| entry_sort(a, b, id));
                }
            }
        }
        log::info!("Apps mapping loop took {:?}", mapping_start.elapsed());

        // Manually insert system apps
        if let Some(installed) = &self.installed {
            for (backend_name, package) in installed {
                if package.id.is_system() {
                    let entry = apps.entry(package.id.clone()).or_default();
                    entry.push(AppEntry {
                        backend_name,
                        info: package.info.clone(),
                        installed: true,
                    });
                    entry.par_sort_unstable_by(|a, b| entry_sort(a, b, &package.id));
                }
            }
        }

        apps.extend(self.catalog_apps.clone());
        self.apps = Arc::new(apps);

        // Update selected sources
        {
            let sources_opt = self.details_page_opt.as_ref().map(|selected| {
                self.selected_sources(selected.backend_name, &selected.id, &selected.info)
            });
            if let Some(sources) = sources_opt {
                if let Some(selected) = &mut self.details_page_opt {
                    selected.sources = sources;
                }
            }
        }

        let duration = start.elapsed();
        log::info!(
            "updated app cache with {} ids in {:?}",
            self.apps.len(),
            duration
        );
    }

    fn update_installed(&self) -> Task<Message> {
        let backends = self.backends.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut installed: Vec<(&'static str, Package)> = backends
                        .par_iter()
                        .flat_map(|(backend_name, backend)| {
                            let start = Instant::now();
                            let mut installed = Vec::new();
                            match backend.installed() {
                                Ok(packages) => {
                                    for package in packages {
                                        installed.push((*backend_name, package));
                                    }
                                }
                                Err(err) => {
                                    log::error!("failed to list installed: {}", err);
                                }
                            }
                            let duration = start.elapsed();
                            log::info!("loaded installed from {} in {:?}", backend_name, duration);
                            installed
                        })
                        .collect();
                    installed.par_sort_unstable_by(|a, b| {
                        let a_is_system = a.1.id.is_system();
                        let b_is_system = b.1.id.is_system();
                        if a_is_system && !b_is_system {
                            cmp::Ordering::Less
                        } else if b_is_system && !a_is_system {
                            cmp::Ordering::Greater
                        } else {
                            LANGUAGE_SORTER.compare(&a.1.info.name, &b.1.info.name)
                        }
                    });
                    let mut installed_results = Vec::new();
                    for (backend_name, package) in &installed {
                        installed_results.push(SearchResult::new(
                            backend_name,
                            package.id.clone(),
                            None,
                            package.info.clone(),
                            0,
                        ));
                    }
                    action::app(Message::Installed(installed))
                })
                .await
                .unwrap_or(action::none())
            },
            |x| x,
        )
    }

    fn update_updates(&self) -> Task<Message> {
        let backends = self.backends.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut updates: Vec<(&'static str, Package)> = backends
                        .par_iter()
                        .flat_map(|(backend_name, backend)| {
                            let start = Instant::now();
                            let mut updates = Vec::new();
                            match backend.updates() {
                                Ok(packages) => {
                                    for package in packages {
                                        updates.push((*backend_name, package));
                                    }
                                }
                                Err(err) => {
                                    log::error!("failed to list updates: {}", err);
                                }
                            }
                            let duration = start.elapsed();
                            log::info!("loaded updates from {} in {:?}", backend_name, duration);
                            updates
                        })
                        .collect();
                    updates.par_sort_unstable_by(|a, b| {
                        if a.1.id.is_system() {
                            cmp::Ordering::Less
                        } else if b.1.id.is_system() {
                            cmp::Ordering::Greater
                        } else {
                            LANGUAGE_SORTER.compare(&a.1.info.name, &b.1.info.name)
                        }
                    });
                    action::app(Message::Updates(updates))
                })
                .await
                .unwrap_or(action::none())
            },
            |x| x,
        )
    }

    fn update_title(&mut self) -> Task<Message> {
        if let Some(window_id) = &self.core.main_window_id() {
            self.set_window_title(fl!("app-name"), *window_id)
        } else {
            Task::none()
        }
    }

    pub(crate) fn operations(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing {
            space_xs, space_m, ..
        } = theme::active().cosmic().spacing;

        let mut children = Vec::new();

        //TODO: get height from theme?
        let progress_bar_height = Length::Fixed(4.0);

        if !self.pending_operations.is_empty() {
            let mut section = widget::settings::section().title(fl!("pending"));
            for (op, progress) in self.pending_operations.values().rev() {
                section = section.add(widget::column::with_children(vec![
                    widget::progress_bar(0.0..=100.0, *progress)
                        .height(progress_bar_height)
                        .into(),
                    widget::Space::with_height(space_xs).into(),
                    widget::text(op.pending_text(*progress as i32)).into(),
                ]));
            }
            children.push(section.into());
        }

        if !self.failed_operations.is_empty() {
            let mut section = widget::settings::section().title(fl!("failed"));
            for (_id, (op, progress, error)) in self.failed_operations.iter().rev() {
                section = section.add(widget::column::with_children(vec![
                    widget::text(op.pending_text(*progress as i32)).into(),
                    widget::text(error).into(),
                ]));
            }
            children.push(section.into());
        }

        if !self.complete_operations.is_empty() {
            let mut section = widget::settings::section().title(fl!("complete"));
            for (_id, op) in self.complete_operations.iter().rev() {
                section = section.add(widget::text(op.completed_text()));
            }
            children.push(section.into());
        }

        if children.is_empty() {
            children.push(widget::text::body(fl!("no-operations")).into());
        }

        widget::column::with_children(children)
            .spacing(space_m)
            .into()
    }

    pub(crate) fn settings(&self) -> Element<'_, Message> {
        let app_theme_selected = match self.config.app_theme {
            AppTheme::Dark => 1,
            AppTheme::Light => 2,
            AppTheme::System => 0,
        };
        widget::settings::view_column(vec![
            widget::settings::section()
                .title(fl!("appearance"))
                .add(
                    widget::settings::item::builder(fl!("theme")).control(widget::dropdown(
                        &self.app_themes,
                        Some(app_theme_selected),
                        move |index| {
                            Message::AppTheme(match index {
                                1 => AppTheme::Dark,
                                2 => AppTheme::Light,
                                _ => AppTheme::System,
                            })
                        },
                    )),
                )
                .into(),
            widget::settings::section()
                .title(fl!("compatibility-estimates"))
                .add(widget::text::body(fl!("compatibility-estimates-help")))
                .add(
                    widget::settings::item::builder(fl!("wayland-filter")).control(
                        widget::dropdown(
                            &self.wayland_filter_options,
                            Some(self.wayland_filter as usize),
                            |i| {
                                Message::WaylandFilter(match i {
                                    1 => WaylandFilter::Excellent,
                                    2 => WaylandFilter::Good,
                                    3 => WaylandFilter::Caution,
                                    4 => WaylandFilter::Limited,
                                    5 => WaylandFilter::Unknown,
                                    _ => WaylandFilter::All,
                                })
                            },
                        ),
                    ),
                )
                .into(),
        ])
        .into()
    }

    pub(crate) fn release_notes(&self, index: usize) -> Element<'_, Message> {
        let (version, date, summary, url) = {
            self.updates
                .as_deref()
                .and_then(|updates| updates.get(index).map(|(_, package)| package))
                .and_then(|selected| {
                    selected.info.releases.last().map(|latest| {
                        (
                            &*latest.version,
                            latest.timestamp,
                            latest.description.to_owned(),
                            latest.url.as_deref(),
                        )
                    })
                })
                .unwrap_or(("", None, None, None))
        };
        let cosmic_theme::Spacing { space_s, .. } = theme::active().cosmic().spacing;
        widget::column::with_capacity(3)
            .push(
                widget::column::with_capacity(2)
                    .push(widget::text::title4(format!(
                        "{} {}",
                        fl!("latest-version"),
                        version
                    )))
                    .push_maybe(
                        date.and_then(|secs| {
                            chrono::DateTime::from_timestamp(secs, 0).map(|dt| {
                                dt.with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d")
                                    .to_string()
                            })
                        })
                        .map(widget::text),
                    ),
            )
            .push(widget::scrollable(widget::text(
                summary.unwrap_or_else(|| fl!("no-description")),
            )))
            .push_maybe(url.map(widget::text))
            .width(Length::Fill)
            .spacing(space_s)
            .into()
    }

    fn sources(&self) -> Vec<Source> {
        let mut sources = Vec::new();
        if self.backends.contains_key("flatpak-user") {
            sources.push(Source {
                backend_name: "flatpak-user",
                id: "flathub".to_string(),
                name: "Flathub".to_string(),
                kind: SourceKind::Recommended {
                    data: include_bytes!("../../res/flathub.flatpakrepo"),
                    enabled: false,
                },
                requires: Vec::new(),
            });
            sources.push(Source {
                backend_name: "flatpak-user",
                id: "cosmic".to_string(),
                name: "COSMIC Flatpak".to_string(),
                kind: SourceKind::Recommended {
                    data: include_bytes!("../../res/cosmic.flatpakrepo"),
                    enabled: false,
                },
                //TODO: can this be defined in flatpakrepo file?
                requires: vec!["flathub".to_string()],
            });
        }

        //TODO: check source URL?
        for (backend_name, backend) in self.backends.iter() {
            for cache in backend.info_caches() {
                let mut found_source = false;
                for source in sources.iter_mut() {
                    if *backend_name == source.backend_name && cache.source_id == source.id {
                        match &mut source.kind {
                            SourceKind::Recommended { enabled, .. } => {
                                *enabled = true;
                            }
                            SourceKind::Custom => {}
                        }
                        found_source = true;
                    }
                }
                //TODO: allow other backends to show sources?
                if !found_source && *backend_name == "flatpak-user" {
                    sources.push(Source {
                        backend_name,
                        id: cache.source_id.clone(),
                        name: cache.source_name.clone(),
                        kind: SourceKind::Custom,
                        requires: Vec::new(),
                    })
                }
            }
        }

        sources
    }

    pub(crate) fn repositories(&self) -> Element<'_, Message> {
        if !cfg!(feature = "flatpak") {
            return widget::text(fl!("no-flatpak")).into();
        }

        let sources = self.sources();
        let mut recommended = widget::settings::section().title(fl!("recommended-flatpak-sources"));
        let mut custom = widget::settings::section().header(widget::column::with_children(vec![
            widget::text::heading(fl!("custom-flatpak-sources")).into(),
            widget::text::body(fl!("import-flatpakrepo")).into(),
        ]));

        let mut has_custom_sources = false;

        for source in sources.iter() {
            let mut adds = Vec::new();
            let mut rms = Vec::new();
            if let Some(add) = source.add() {
                adds.push(add);
            }
            if let Some(rm) = source.remove() {
                rms.push(rm);
            }
            for other in sources.iter() {
                if source.backend_name == other.backend_name {
                    // Add other sources required by this source
                    if source.requires.contains(&other.id) {
                        if let Some(add) = other.add() {
                            adds.push(add);
                        }
                    }

                    // Remove other sources that require this source
                    if other.requires.contains(&source.id) {
                        if let Some(rm) = other.remove() {
                            rms.push(rm);
                        }
                    }
                }
            }

            let item =
                widget::settings::item::builder(source.name.clone()).description(source.id.clone());
            let element = match self
                .repos_changing
                .iter()
                .find(|x| x.0 == source.backend_name && x.1 == source.id)
                .map(|x| x.2)
            {
                Some(adding) => item.control(widget::text(if adding {
                    fl!("adding")
                } else {
                    fl!("removing")
                })),
                None => {
                    if !adds.is_empty() {
                        item.control(widget::button::text(fl!("add")).on_press_maybe(
                            if self.repos_changing.is_empty() {
                                Some(Message::RepositoryAdd(source.backend_name, adds.clone()))
                            } else {
                                None
                            },
                        ))
                    } else if !rms.is_empty() {
                        item.control(widget::button::text(fl!("remove")).on_press_maybe(
                            if self.repos_changing.is_empty() {
                                Some(Message::RepositoryRemove(source.backend_name, rms.clone()))
                            } else {
                                None
                            },
                        ))
                    } else {
                        item.control(widget::horizontal_space())
                    }
                }
            };

            match source.kind {
                SourceKind::Recommended { .. } => {
                    recommended = recommended.add(element);
                }
                SourceKind::Custom => {
                    has_custom_sources = true;
                    custom = custom.add(element);
                }
            }
        }
        // Add list item when no custom sources exist
        if !has_custom_sources {
            custom = custom.add(widget::text::body(fl!("no-custom-flatpak-sources")));
        }

        let custom = widget::column::with_children(vec![
            custom.into(),
            widget::container(widget::button::standard(fl!("import")).on_press_maybe(
                if self.repos_changing.is_empty() {
                    Some(Message::RepositoryAddDialog("flatpak-user"))
                } else {
                    None
                },
            ))
            .width(Length::Fill)
            .align_x(Alignment::End)
            .into(),
        ])
        .spacing(theme::spacing().space_xxs);

        widget::settings::view_column(vec![recommended.into(), custom.into()]).into()
    }

    fn view_search_results<'a>(
        &'a self,
        input: &str,
        results: &'a [SearchResult],
        spacing: cosmic_theme::Spacing,
        grid_width: usize,
    ) -> Element<'a, Message> {
        views::render_search_results(
            input,
            results,
            self.results_limit,
            spacing,
            grid_width,
            &self.app_stats,
        )
    }

    fn view_explore_page<'a>(
        &'a self,
        spacing: cosmic_theme::Spacing,
        grid_width: usize,
        viewport_height: f32,
    ) -> Element<'a, Message> {
        views::render_explore_page(
            &self.explore_page_opt,
            &self.explore_results,
            self.loading_frame,
            self.explore_loading || self.backends.is_empty(),
            self.results_limit,
            spacing,
            grid_width,
            viewport_height,
            &self.app_stats,
        )
    }

    fn view_installed_page<'a>(
        &'a self,
        spacing: cosmic_theme::Spacing,
        grid_width: usize,
    ) -> Element<'a, Message> {
        views::render_installed_page(
            &self.installed_results,
            spacing,
            grid_width,
            &self.app_stats,
        )
    }

    fn view_updates_page<'a>(
        &'a self,
        spacing: cosmic_theme::Spacing,
        grid_width: usize,
    ) -> Element<'a, Message> {
        views::render_updates_page(
            &self.updates,
            &self.waiting_installed,
            &self.waiting_updates,
            &self.pending_operations,
            spacing,
            grid_width,
            &self.app_stats,
        )
    }

    fn view_category_page<'a>(
        &'a self,
        nav_page: NavPage,
        spacing: cosmic_theme::Spacing,
        grid_width: usize,
    ) -> Element<'a, Message> {
        views::render_category_page(
            nav_page,
            &self.category_results,
            &self.subcategory_options,
            self.subcategory,
            self.explore_results
                .get(&ExplorePage::LinuxGames)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            &self.sources(),
            self.results_limit,
            spacing,
            grid_width,
            &self.app_stats,
        )
    }

    fn view_responsive(&self, size: Size) -> Element<'_, Message> {
        self.size.set(Some(size));
        let spacing = theme::active().cosmic().spacing;
        let cosmic_theme::Spacing { space_s, .. } = spacing;
        let grid_width = (size.width - 2.0 * space_s as f32).floor().max(0.0) as usize;

        let content = match &self.details_page_opt {
            Some(details_page) => details_page.view(self, spacing, grid_width, &self.app_stats),
            None => match &self.search_results {
                Some((input, results)) => {
                    self.view_search_results(input, results, spacing, grid_width)
                }
                None => match self
                    .nav_model
                    .active_data::<NavPage>()
                    .map_or(NavPage::default(), |nav_page| *nav_page)
                {
                    NavPage::Explore => self.view_explore_page(spacing, grid_width, size.height),
                    NavPage::Installed => self.view_installed_page(spacing, grid_width),
                    //TODO: reduce duplication
                    NavPage::Updates => self.view_updates_page(spacing, grid_width),
                    nav_page => self.view_category_page(nav_page, spacing, grid_width),
                },
            },
        };
        let nav = self
            .nav_model
            .active_data::<NavPage>()
            .copied()
            .unwrap_or_default();
        if self.details_page_opt.is_some()
            || (self.search_results.is_none()
                && matches!(nav, NavPage::Installed | NavPage::Updates))
        {
            return content;
        }
        let source = widget::column::with_children(vec![
            widget::text::caption(fl!("filter-source")).into(),
            widget::dropdown(
                &self.store_source_options,
                Some(self.store_source as usize),
                |i| {
                    Message::StoreSource(match i {
                        1 => crate::search::StoreSource::System,
                        2 => crate::search::StoreSource::Flatpak,
                        3 => crate::search::StoreSource::Steam,
                        _ => crate::search::StoreSource::All,
                    })
                },
            )
            .into(),
        ])
        .spacing(4);
        let sort = widget::column::with_children(vec![
            widget::text::caption(fl!("filter-sort")).into(),
            widget::dropdown(
                &self.search_sort_options,
                Some(self.search_sort_mode as usize),
                |i| {
                    Message::SearchSortMode(match i {
                        1 => SearchSortMode::MostDownloads,
                        2 => SearchSortMode::RecentlyUpdated,
                        3 => SearchSortMode::BestWaylandSupport,
                        4 => SearchSortMode::Name,
                        5 => SearchSortMode::NewestGames,
                        _ => SearchSortMode::Relevance,
                    })
                },
            )
            .into(),
        ])
        .spacing(4);
        let filters: Element<_> = if size.width >= 600.0 {
            widget::row::with_children(vec![source.into(), sort.into()])
                .spacing(24)
                .into()
        } else {
            widget::column::with_children(vec![source.into(), sort.into()])
                .spacing(12)
                .into()
        };
        let compatibility = widget::tooltip(
            widget::checkbox(fl!("native-linux-only"), self.native_only)
                .on_toggle(Message::NativeOnly),
            widget::text::body(fl!("native-linux-help")),
            widget::tooltip::Position::Bottom,
        );
        let controls = widget::column::with_children(vec![
            filters,
            widget::row::with_children(vec![
                compatibility.into(),
                widget::horizontal_space().into(),
                widget::button::text(fl!("reset-filters"))
                    .on_press(Message::ResetFilters)
                    .into(),
            ])
            .align_y(cosmic::iced::Alignment::Center)
            .into(),
        ])
        .spacing(12)
        .padding([12, 16]);
        let mut page = widget::column::with_capacity(3).push(controls);
        if self.search_pending && self.search_active {
            page = page.push(
                widget::container(widget::text::caption(fl!("search-steam-loading")))
                    .padding([0, 16]),
            );
        }
        page.push(content).spacing(8).into()
    }
}

/// Implement [`Application`] to integrate with COSMIC.
impl Application for App {
    /// Multithreaded async executor to use with the app.
    type Executor = executor::multi::Executor;

    /// Argument received
    type Flags = Flags;

    /// Message type specific to our [`App`].
    type Message = Message;

    /// The unique application ID to supply to the window manager.
    const APP_ID: &'static str = "app.shipdocs.Kompas";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    /// Creates the application, and optionally emits command on initialize.
    fn init(core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let locale = sys_locale::get_locale().unwrap_or_else(|| {
            log::warn!("failed to get system locale, falling back to en-US");
            String::from("en-US")
        });

        let os_codename = OsInfo::detect()
            .map(|info| info.codename().to_string())
            .unwrap_or_else(|e| {
                log::warn!("failed to detect OS codename: {}", e);
                String::new()
            });

        let app_themes = vec![fl!("match-desktop"), fl!("dark"), fl!("light")];
        let search_sort_options = vec![
            fl!("sort-relevance"),
            fl!("sort-popular"),
            fl!("sort-recent"),
            fl!("sort-wayland"),
            fl!("sort-name"),
            fl!("sort-newest-games"),
        ];
        let wayland_filter_options = vec![
            fl!("filter-all"),
            fl!("filter-excellent"),
            fl!("filter-good"),
            fl!("filter-caution"),
            fl!("filter-limited"),
            fl!("filter-unknown"),
        ];

        let mut nav_model = widget::nav_bar::Model::default();
        let mut nav_ids = Vec::new();
        for &nav_page in NavPage::all() {
            if nav_page == NavPage::Applets && !crate::pages::cosmic_desktop() {
                continue;
            }
            let id = nav_model
                .insert()
                .icon(nav_page.icon())
                .text(nav_page.title())
                .data::<NavPage>(nav_page)
                .id();
            nav_ids.push((nav_page, id));
            if nav_page == NavPage::default() {
                //TODO: save last page?
                nav_model.activate(id);
            }
        }

        // Build buttons for applet placement dialog

        let mut applet_placement_buttons =
            cosmic::widget::segmented_button::SingleSelectModel::builder().build();
        let _ = applet_placement_buttons.insert().text(fl!("panel")).id();
        let _ = applet_placement_buttons.insert().text(fl!("dock")).id();
        applet_placement_buttons.activate_position(0);

        let mut app = App {
            core,
            config_handler: flags.config_handler,
            config: flags.config,
            mode: flags.mode,
            locale,
            os_codename,
            app_themes,
            apps: Arc::new(Apps::new()),
            catalog_apps: Apps::new(),
            backends: Backends::new(),
            context_page: ContextPage::Settings,
            dialog_pages: VecDeque::new(),
            explore_page_opt: None,
            key_binds: key_binds(),
            nav_model,
            nav_ids,
            nav_history: Vec::new(),
            subcategory: 0,
            subcategory_options: vec![fl!("subcategory-all")],
            #[cfg(feature = "notify")]
            notification_opt: None,
            pending_operation_id: 0,
            pending_operations: BTreeMap::new(),
            progress_operations: BTreeSet::new(),
            complete_operations: BTreeMap::new(),
            failed_operations: BTreeMap::new(),
            repos_changing: Vec::new(),
            scrollable_id: widget::Id::unique(),
            scroll_views: HashMap::new(),
            search_active: false,
            search_id: widget::Id::unique(),
            search_input: String::new(),
            search_sort_mode: SearchSortMode::Relevance,
            store_source: crate::search::StoreSource::All,
            store_source_options: vec![
                fl!("source-all"),
                fl!("system-packages"),
                "Flatpak".to_string(),
                "Steam".to_string(),
            ],
            native_only: true,
            results_limit: crate::constants::MAX_RESULTS,
            search_pending: false,
            explore_loading: true,
            search_sort_options,
            wayland_filter: WaylandFilter::All,
            wayland_filter_options,
            size: Cell::new(None),
            installed: None,
            updates: None,
            waiting_installed: Vec::new(),
            waiting_updates: Vec::new(),
            category_results: None,
            explore_results: HashMap::new(),
            installed_results: None,
            search_results: None,
            details_page_opt: None,
            applet_placement_buttons,
            uninstall_purge_data: false,
            loading_frame: 0,
            app_stats: HashMap::new(),
        };

        if let Some(subcommand) = flags.subcommand_opt {
            // Search for term
            app.search_active = true;
            app.search_input = subcommand;
        }

        match app.mode {
            Mode::Normal => {}
            Mode::GStreamer { .. } => {
                app.core.window.use_template = false;
            }
        }

        let command = Task::batch([
            app.update_title(),
            data::catalog_task(),
            app.update_backends(false),
            Task::perform(
                async move {
                    crate::stats::load_stats_async();
                    crate::stats::load_stats_map()
                },
                |stats| action::app(Message::StatsLoaded(stats)),
            ),
        ]);
        (app, command)
    }

    fn nav_model(&self) -> Option<&widget::nav_bar::Model> {
        match self.mode {
            Mode::GStreamer { .. } => None,
            _ => Some(&self.nav_model),
        }
    }

    #[cfg(feature = "single-instance")]
    fn dbus_activation(&mut self, msg: cosmic::dbus_activation::Message) -> Task<Message> {
        let mut tasks = Vec::with_capacity(2);
        if self.core.main_window_id().is_none() {
            // Create window if required
            let (window_id, task) = window::open(window::Settings {
                min_size: Some(Size::new(420.0, 300.0)),
                decorations: false,
                exit_on_close_request: false,
                ..Default::default()
            });
            self.core.set_main_window_id(Some(window_id));
            tasks.push(task.map(|_id| action::none()));
        }
        if let cosmic::dbus_activation::Details::ActivateAction { action, .. } = msg.msg {
            // Search for term
            self.search_active = true;
            self.search_input = action;
            tasks.push(self.search());
        }
        Task::batch(tasks)
    }

    fn on_app_exit(&mut self) -> Option<Message> {
        Some(Message::WindowClose)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        (self.core.main_window_id() == Some(id)).then_some(Message::WindowClose)
    }

    fn on_escape(&mut self) -> Task<Message> {
        if self.core.window.show_context {
            // Close context drawer if open
            self.core.window.show_context = false;
        } else if self.details_page_opt.is_some() {
            return self.go_back();
        } else if self.search_active {
            // Close search if open
            self.search_active = false;
            if self.search_results.take().is_some() {
                return self.update_scroll();
            }
        }
        Task::none()
    }

    fn on_nav_select(&mut self, id: widget::nav_bar::Id) -> Task<Message> {
        if let Some(page) = self.nav_model.active_data::<NavPage>().copied() {
            if self
                .nav_ids
                .iter()
                .any(|(p, nav_id)| *nav_id == id && *p != page)
            {
                self.nav_history.push((page, self.subcategory));
            }
            if self.nav_history.len() > 32 {
                self.nav_history.remove(0);
            }
        }
        self.subcategory = 0;
        self.results_limit = crate::constants::MAX_RESULTS;
        self.category_results = None;
        self.explore_page_opt = None;
        self.search_active = false;
        self.search_input.clear();
        self.search_pending = false;
        self.search_results = None;
        self.details_page_opt = None;
        self.nav_model.activate(id);
        if self.nav_model.active_data::<NavPage>() == Some(&NavPage::Game) {
            self.search_sort_mode = SearchSortMode::NewestGames;
            if let Some(results) = self.explore_results.get_mut(&ExplorePage::LinuxGames) {
                crate::search_logic::sort_results(results, self.search_sort_mode, &self.app_stats);
            }
        } else if self.search_sort_mode == SearchSortMode::NewestGames {
            self.search_sort_mode = SearchSortMode::Relevance;
        }
        self.subcategory_options = vec![fl!("subcategory-all")];
        if let Some(page) = self.nav_model.active_data::<NavPage>() {
            self.subcategory_options.extend(
                page.subcategories()
                    .iter()
                    .map(|s| crate::localize::LANGUAGE_LOADER.get(s.key)),
            );
        }
        let mut commands = Vec::with_capacity(2);
        self.scroll_views.clear();
        commands.push(self.update_scroll());
        if let Some(categories) = self.active_categories() {
            commands.push(self.categories(categories));
        }
        if let Some(NavPage::Updates) = self.nav_model.active_data::<NavPage>() {
            // Refresh when going to updates page
            commands.push(self.update(Message::CheckUpdates));
        }
        Task::batch(commands)
    }

    /// Handle application events here.
    fn update(&mut self, message: Self::Message) -> Task<Message> {
        handlers::update(self, message)
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match &self.context_page {
            ContextPage::Operations => context_drawer::context_drawer(
                self.operations(),
                Message::ToggleContextPage(ContextPage::Operations),
            )
            .title(fl!("operations")),
            ContextPage::Settings => context_drawer::context_drawer(
                self.settings(),
                Message::ToggleContextPage(ContextPage::Settings),
            )
            .title(fl!("settings")),
            ContextPage::ReleaseNotes(i, app_name) => context_drawer::context_drawer(
                self.release_notes(*i),
                Message::ToggleContextPage(ContextPage::ReleaseNotes(*i, app_name.clone())),
            )
            .title(app_name),
            ContextPage::Repositories => context_drawer::context_drawer(
                self.repositories(),
                Message::ToggleContextPage(ContextPage::Repositories),
            )
            .title(fl!("software-repositories")),
        })
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let dialog_page = self.dialog_pages.front()?;
        views::render_dialog(
            dialog_page,
            &self.failed_operations,
            self.size.get(),
            self.uninstall_purge_data,
            &self.applet_placement_buttons,
            Self::APP_ID,
        )
    }

    fn footer(&self) -> Option<Element<'_, Message>> {
        views::render_footer(
            &self.progress_operations,
            &self.pending_operations,
            &self.complete_operations,
        )
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        views::render_header_start(
            &self.mode,
            &self.search_input,
            self.search_id.clone(),
            self.can_go_back(),
        )
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        views::render_header_end(&self.mode)
    }

    /// Creates a view after each update.
    fn view(&self) -> Element<'_, Self::Message> {
        let content: Element<_> = match &self.mode {
            Mode::Normal => widget::responsive(move |mut size| {
                size.width = size.width.min(MAX_GRID_WIDTH);
                widget::scrollable(
                    widget::container(
                        widget::container(self.view_responsive(size)).max_width(MAX_GRID_WIDTH),
                    )
                    .align_x(Alignment::Center),
                )
                .id(self.scrollable_id.clone())
                .on_scroll(Message::ScrollView)
                .into()
            })
            .into(),
            Mode::GStreamer {
                codec,
                selected,
                installing,
            } => views::render_gstreamer_view(
                codec,
                selected,
                *installing,
                &self.pending_operations,
                &self.failed_operations,
                &self.complete_operations,
                &self.search_results,
            ),
        };

        // Uncomment to debug layout:
        //content.explain(cosmic::iced::Color::WHITE)
        content
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        // When closing the main window, view_window may be called after the main window is unset
        widget::horizontal_space().into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        handlers::subscription(self)
    }
}
