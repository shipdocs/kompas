//! Application-wide constants

/// Icon size for search results (48x48 pixels)
pub const ICON_SIZE_SEARCH: u16 = 48;

/// Icon size for package cards (64x64 pixels)
pub const ICON_SIZE_PACKAGE: u16 = 64;

/// Icon size for detail view (128x128 pixels)
pub const ICON_SIZE_DETAILS: u16 = 128;

/// Maximum width for responsive grid layout
pub const MAX_GRID_WIDTH: f32 = 1600.0;

/// Maximum number of search results to display
pub const MAX_RESULTS: usize = 100;

/// Directory name under the XDG cache dir for everything Kompas caches.
pub const CACHE_DIR: &str = "kompas";
