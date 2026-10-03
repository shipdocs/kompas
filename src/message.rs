use cosmic::widget;
use cosmic::{
    cosmic_theme,
    iced::core::SmolStr,
    iced::keyboard::{Key, Modifiers},
    iced::widget::scrollable,
};
use std::sync::{Arc, Mutex};

use crate::app_id::AppId;
use crate::app_info::AppInfo;
use crate::backend::{Backends, Package};
use crate::category::Category;
use crate::config::{AppTheme, Config};
use crate::gstreamer::GStreamerExitCode;
use crate::operation::{OperationKind, RepositoryAdd, RepositoryRemove};
use crate::pages::{ContextPage, DialogPage, ExplorePage};
use crate::search::{SearchResult, SearchSortMode, WaylandFilter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    SearchActivate,
    Back,
}

impl Action {
    pub fn message(&self) -> Message {
        match self {
            Self::SearchActivate => Message::SearchActivate,
            Self::Back => Message::Back,
        }
    }
}

/// Messages that are used specifically by our [`App`](crate::App).
#[derive(Clone, Debug)]
pub enum Message {
    AppTheme(AppTheme),
    Backends(Backends),
    StatsLoaded(
        (
            std::collections::HashMap<crate::AppId, u64>,
            std::collections::HashMap<crate::AppId, crate::app_info::WaylandCompatibility>,
        ),
    ),
    Apps(Arc<crate::app_entry::Apps>),
    CatalogLoaded(crate::app_entry::Apps),
    CatalogImagesReady,
    Back,
    Subcategory(usize),
    CategoryResults(&'static [Category], Vec<SearchResult>),
    CheckUpdates,
    Config(Config),
    DialogCancel,
    DialogConfirm,
    DialogPage(DialogPage),
    ExplorePage(Option<ExplorePage>),
    ExploreResults(ExplorePage, Vec<SearchResult>),
    ExploreResultsReady(std::collections::HashMap<ExplorePage, Vec<SearchResult>>),
    GStreamerExit(GStreamerExitCode),
    GStreamerInstall,
    GStreamerToggle(usize),
    Installed(Vec<(&'static str, Package)>),
    InstalledResults(Vec<SearchResult>),
    Key(Modifiers, Key, Option<SmolStr>),
    LaunchUrl(String),
    MaybeExit,
    LoadingTick,
    #[cfg(feature = "notify")]
    Notification(Arc<Mutex<notify_rust::NotificationHandle>>),
    OpenDesktopId(String),
    Operation(OperationKind, &'static str, AppId, Arc<AppInfo>),
    PendingComplete(u64),
    PendingDismiss,
    PendingError(u64, String),
    PendingProgress(u64, f32),
    RepositoryAdd(&'static str, Vec<RepositoryAdd>),
    RepositoryAddDialog(&'static str),
    RepositoryRemove(&'static str, Vec<RepositoryRemove>),
    ScrollView(scrollable::Viewport),
    SearchActivate,
    SearchClear,
    SearchInput(String),
    SearchResults(String, Vec<SearchResult>, bool),
    SearchAugmented(String, Vec<SearchResult>),
    SearchProgress(String),
    SearchSortMode(SearchSortMode),
    StoreSource(crate::search::StoreSource),
    NativeOnly(bool),
    ResetFilters,
    ShowMore,
    FindApp(String),
    SearchSubmit(String),
    WaylandFilter(WaylandFilter),
    Select(
        &'static str,
        AppId,
        Option<widget::icon::Handle>,
        Arc<AppInfo>,
    ),
    SelectInstalled(usize),
    SelectUpdates(usize),
    SelectNone,
    SelectCategoryResult(usize),
    SelectExploreResult(ExplorePage, usize),
    SelectSearchResult(usize),
    SelectedAddonsViewMore(bool),
    SelectedScreenshot(usize, String, Vec<u8>),
    SelectedScreenshotShown(usize),
    ToggleUninstallPurgeData(bool),
    SelectedSource(usize),
    SystemThemeModeChange(cosmic_theme::ThemeMode),
    ToggleContextPage(ContextPage),
    UpdateAll,
    Updates(Vec<(&'static str, Package)>),
    WindowClose,
    WindowCloseRequested(cosmic::iced::window::Id),
    WindowNew,
    SelectPlacement(cosmic::widget::segmented_button::Entity),
    PlaceApplet(AppId),
}
