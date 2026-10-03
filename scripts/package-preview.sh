#!/usr/bin/env bash
# Package an already built binary for Zorin 18 / Ubuntu 24.04 testing.
set -euo pipefail
binary=${1:-target/debug/kompas}
output=${2:-dist}
test -x "$binary"
mkdir -p "$output"
output=$(realpath "$output")
architecture=$(dpkg --print-architecture)
version="0.1.0+kompas.$(date -u +%Y%m%d%H%M%S).$(git rev-parse --short HEAD)"
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
install -Dm0755 "$binary" "$staging/usr/bin/kompas"
strip "$staging/usr/bin/kompas"
# Existing scripts may still use the old command.
ln -s kompas "$staging/usr/bin/cosmic-store"
install -Dm0644 res/app.shipdocs.Kompas.desktop "$staging/usr/share/applications/app.shipdocs.Kompas.desktop"
install -Dm0644 res/app.shipdocs.Kompas.metainfo.xml "$staging/usr/share/metainfo/app.shipdocs.Kompas.metainfo.xml"
while IFS= read -r icon; do
    install -Dm0644 "$icon" "$staging/usr/share/icons/${icon#res/icons/}"
done < <(find res/icons/hicolor -type f -name '*.svg')
install -Dm0644 LICENSE "$staging/usr/share/doc/kompas/copyright"
install -Dm0644 patches/iced_wgpu/LICENSE "$staging/usr/share/doc/kompas/iced-wgpu-license"
install -Dm0644 patches/iced_wgpu/KOMPAS-PATCH.md "$staging/usr/share/doc/kompas/renderer-patch.md"
install -Dm0644 patches/iced_winit/LICENSE "$staging/usr/share/doc/kompas/iced-winit-license"
install -Dm0644 patches/iced_winit/KOMPAS-PATCH.md "$staging/usr/share/doc/kompas/runtime-patch.md"
# Ship a package-linked catalog so Software can match the app name, icon and license.
python3 scripts/package-metadata.py "$staging/usr/share" "$version"
mkdir -p "$staging/DEBIAN"
# Resolve the actual binary's linked libraries on the target Ubuntu release.
dependencies=$(dpkg-shlibdeps -O -e"$staging/usr/bin/kompas" | sed -n 's/^shlibs:Depends=//p')
test -n "$dependencies"
cat > "$staging/DEBIAN/control" <<EOF
Package: kompas
Provides: cosmic-store
Conflicts: cosmic-store
Replaces: cosmic-store
Version: $version
Installed-Size: $(du -sk "$staging/usr" | cut -f1)
Architecture: $architecture
Maintainer: ShipDocs <info@shipdocs.app>
Section: admin
Priority: optional
Depends: $dependencies, libxkbcommon-x11-0, apt-config-icons, apt-config-icons-hidpi, apt-config-icons-large, apt-config-icons-large-hidpi, adwaita-icon-theme
Recommends: flatpak, packagekit
Homepage: https://github.com/shipdocs/kompas
Description: Discover apps and games in one place
 Kompas brings together Linux apps from your system and Flathub,
 alongside games from Steam. Browse categories and game genres,
 search by name, and filter by source or native Linux support.
 .
 Install system and Flatpak apps directly, or continue in Steam.
 Free and open source (GPL-3.0-only). Preview for Zorin 18 and Ubuntu 24.04.
EOF
package="$output/kompas_${version}_${architecture}.deb"
dpkg-deb --build --root-owner-group "$staging" "$package"
(cd "$output" && sha256sum "$(basename "$package")" > SHA256SUMS)
printf 'Created %s\n' "$package"
