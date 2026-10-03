# Kompas

A unified software store for Zorin and Ubuntu. Discover applications and games from your configured system repositories, Flatpak sources and Steam in one interface, with filters that help you find relevant software.

Kompas is independently maintained by ShipDocs and built on [COSMIC Store](https://github.com/pop-os/cosmic-store). It is a preview for user testing, not an official Zorin product. The interface defaults to English and includes Dutch translations.

The program and package are named `kompas`. A `cosmic-store` command alias is retained for existing scripts. The app ID is `app.shipdocs.Kompas`; old preferences and file associations are preserved. This is a ShipDocs project, not an official Zorin store.

[Project website](https://shipdocs.github.io/kompas/) · [Releases](https://github.com/shipdocs/kompas/releases)

## Install the Zorin test build

Download the `.deb` and checksums from the newest [preview release](https://github.com/shipdocs/kompas/releases).
For the newest development package, download **kompas-zorin-preview-amd64** from the latest successful
[development build](https://github.com/shipdocs/kompas/actions/workflows/lint.yml),
extract the ZIP and install the `.deb` on Zorin 18 / Ubuntu 24.04 (64-bit Intel/AMD):

```bash
sudo apt install ./kompas_*.deb
```

Open **Kompas** from the application menu or run `kompas`. Installation replaces
the previous `cosmic-store` package automatically. Packages
are optimized release builds with debug symbols stripped.
The SHA256SUMS file accompanies each package. The installed package is exercised
under X11 by CI before the workflow succeeds.

For Flatpak results, ensure Flathub is configured (see below). Kompas uses your
existing remotes and does not silently add software sources.

### Opening the package in Zorin Software

Before the first installation, Zorin Software may show a generic package page with
“Unknown License”, no release details and the download size labelled as installed
size. Its local-DEB backend does not read the AppStream metadata inside an
uninstalled package. Kompas includes GPL-3.0-only license information, AppStream
metadata and a package description that explicitly states the license.

If Software shows a description from an older preview, close it completely with
`gnome-software --quit` and reopen the package. Alternatively, install with the
terminal command above. After installation, restart Software so it can load the
installed Kompas catalog. CI captures both the first-install page on a clean
system and the page after installation; those are distinct checks.

## Features

- **Wayland Compatibility**: Shows badges and risk estimates derived from AppStream fields, Flatpak permissions, and framework heuristics. These estimates are not verified compatibility tests.
- **Find relevant software**: Search, categories and subcategories, source filters, alphabetical sorting, popularity and recent updates.
- **Fresh game discovery**: Game sections default to newest original release date. Games without a known release date appear below dated games; a new package update does not make an old game a new release.
- **Easy navigation**: A visible back button, Alt+Left and Escape return to your previous results.
- **Unified discovery**: System/Zorin packages, Flatpak and Steam with source selection, Linux-native filtering and consistent sorting.
- **Complete browsing**: Progressive “Show more” browsing through the loaded catalog, result counts and recoverable empty states.
- **Game launchers**: Discover Steam and Heroic (Epic/GOG); Steam game pages help install the client before handing over installation.
- **Performance**: AppStream data is parsed in the background and icons are cached. Release packages are optimized builds; a debug build from `cargo build` is several times slower.

## How discovery works

The start page shows Steam games with current native Linux metadata, followed by new releases and general games. A small curated set provides discovery entry points; names, prices, artwork and Linux flags are fetched from Steam, and games without an explicit current Linux flag are excluded from the native section.
Steam artwork, storefront pricing, controller metadata and Linux platform flags come
from Steam's public store endpoints. These endpoints are not a guaranteed stable API.
The Netherlands region is used for displayed prices. Cached featured metadata remains
available if Steam cannot be reached. Local applications remain usable without Steam.

Search first displays results from configured Flatpak and system sources, then adds
Steam matches after a short typing debounce. Plain text queries of at least two
characters are sent to Steam when the Wayland filter is set to All. URI/file/codec
searches remain local. Search GTA expands to Grand Theft Auto; Photoshop, Premiere
and Microsoft Office searches also suggest available native alternatives.

System software is resolved through PackageKit against enabled repositories, including
Zorin's own repositories. Metadata origins no longer need to contain an Ubuntu codename.
The store does not add repositories or expand package permissions automatically.

Steam games have separate actions to open the installation dialog in Steam, view/buy
in the web store, and check ProtonDB. Installation requires a working Steam URI handler
and any required game license. Steam artwork is cached locally; an empty cache displays
a game icon until images arrive. This does not claim that every Windows game or online
mode works on Linux. No Epic/GOG login, purchase or account linking is performed.

### Unified browsing and compatibility

“All apps” lists applications from the configured system/Flatpak catalogs and the fetched Steam featured selection. Live search extends Steam discovery; this is not an exhaustive local index of Steam. The source selector applies to search, category lists and home sections, and can choose a Flatpak alternative when the preferred source is a system package. All result sources share sorting, including Name (A–Z). Popularity and update sorts place items with missing metadata after items with known values; Steam sales are not converted to Flatpak download counts.

Native Linux only is enabled by default. It hides Steam titles unless Steam explicitly reports a native Linux version. Disable it to include titles requiring a Proton compatibility check. PackageKit availability is checked against the configured system; Flatpak catalogs are selected by libflatpak for the host architecture. Native support does not establish that a particular GPU, driver, RAM configuration, anti-cheat setup or desktop session meets an app's requirements. Hardware/Proton compatibility inference remains future work. This conservative default intentionally hides many Windows games that can run well with Proton.

The user-facing product name is **Kompas**, a working name rather than a cleared trademark. The executable and Debian package are `kompas`. The app ID is `app.shipdocs.Kompas`, so Kompas does not share COSMIC Store reviews. Legacy preferences are loaded on first use and a hidden desktop alias preserves existing file associations; the old command is a compatibility alias. Window controls use Adwaita symbolic icons, which are a package dependency.

Malformed YAML components are skipped individually so one duplicate translation key cannot discard an entire system repository. A repository header with a repeated key (seen in Zorin's extra catalog) is tolerated, and that catalog's origin is then unknown; any other invalid header skips the file with a warning. Existing AppStream caches are rebuilt once for this parser revision.

Kompas stores its caches in `~/.cache/kompas`. Earlier previews used `~/.cache/cosmic-store`; that directory is no longer read and can be deleted.

Steam discovery validates product types through cached app-details metadata before publishing items. Hardware and video products are excluded even when storefront search calls them apps. Product facts are cached for a day, with previously validated metadata as an offline fallback. First-time remote results may arrive later because this requires additional background requests; local results remain available immediately.

## Branch Structure

- `master`: Historical upstream baseline (pop-os/cosmic-store).
- `develop`: Active development branch containing all enhancements.

## Build and Run on Zorin / Ubuntu

Use a current stable Rust toolchain installed through [rustup](https://rustup.rs/).
The manifest requires Rust 1.85 or newer; locked dependencies and workspace tools
may require a newer compiler. The distribution's Rust package may be too old.

Install the native build dependencies:

```bash
sudo apt update
sudo apt install build-essential git pkg-config libflatpak-dev libssl-dev \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig1-dev libegl1-mesa-dev
```

Clone and build the development branch without changing the lockfile:

```bash
git clone --branch develop https://github.com/shipdocs/kompas.git
cd kompas
rustup update stable
cargo +stable build --release --locked
cargo +stable run --release --locked
```

Run the store as your normal desktop user, without `sudo`. It uses the system's
configured Flatpak remotes and PackageKit service. For Flatpak apps, ensure
Flatpak and Flathub are available:

```bash
sudo apt install flatpak packagekit
flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
```

The default build includes both Flatpak and PackageKit. It uses libcosmic's winit
backend; a COSMIC desktop session is not required by the build instructions.
Actual startup and rendering must still be tested on your Zorin X11/Wayland session.

For startup diagnostics:

```bash
RUST_LOG=kompas=info RUST_BACKTRACE=1 cargo +stable run --release --locked
```

The Debian packaging no longer requires Pop!_OS-specific `appstream-data-pop` or
`cosmic-icons` packages. CI validates package upgrades, AppStream metadata, app navigation and the package page
in GNOME Software under X11. Your testing on a real Zorin installation is still needed. The instructions above run directly from the build
directory and do not replace Zorin Software or Bazaar.

A local Debian build uses the existing vendoring recipe and requires `debhelper`
and `just` (at least 1.13; a current release is recommended):

```bash
dpkg-buildpackage -us -uc -b
```

Use this after the source build and checks succeed. For a lighter preview package
from an existing build, run `bash scripts/package-preview.sh target/release/kompas`.
CI publishes an installable preview package for testing.

## Development checks

```bash
cargo +stable fmt -- --check
cargo +stable clippy --locked -- -D warnings
cargo +stable test --locked --workspace
```

Pull requests into `develop` and pushes to `develop` run these checks on Ubuntu 24.04.

## Current limitations

- Release-date sorting depends on publisher metadata. Unknown dates go last; first-added dates for Flatpak/system apps are not available yet.
- Download counts indicate popularity within their source, not quality or comparable sales across stores. A user review system is planned, not implemented; Kompas does not inherit COSMIC Store ratings.
- Steam discovery and search are integrated and only available on x86_64. Epic and GOG catalogs remain future work.
- Steam controls purchase and installation; ownership is not checked by this store.
- ProtonDB opens as an external compatibility reference; compatibility ratings are not fetched or asserted.
- Wayland badges are estimates; they do not certify GPU, controller, or runtime compatibility.

## Contributing

Submit pull requests against `develop`. Include the user-visible change and relevant
validation. For discovery, ranking and review design, see
[the store design review](docs/store-design-review.md).

## Zorin distribution

The preview `.deb` can be installed on an existing Zorin system. Inclusion in the
standard Zorin installation requires a stable release, maintained distribution and
agreement with the Zorin team. A signed APT repository would provide automatic
updates; it has not been published yet.
