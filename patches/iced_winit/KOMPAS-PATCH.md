# Runtime patch

Source: pop-os/iced at 10db38f982001a714bd94e99a082368762b378ee, the iced submodule of the locked libcosmic 3b8ad45950f5d23c8550e18e628f6e70b7089d89 (`winit/`). MIT license and attribution are preserved. The standalone manifest resolves workspace dependencies against the same locked sources.

One functional change in src/program.rs (`Event::WindowCreated`): iced creates an invisible bootstrap window to initialise the renderer and hands it to the clipboard. Mutter (Zorin/Ubuntu GNOME) ignores the hidden flag, so a titleless "winit window" appeared in the taskbar for the whole session and could not be closed. The clipboard now moves to the first real window when its current window is not one the window manager tracks, which drops the bootstrap window.

Verify with `WAYLAND_DEBUG=client kompas`: exactly one `xdg_toplevel` (`set_app_id("app.shipdocs.Kompas")`) must be created. Remove this patch when libcosmic stops keeping the bootstrap window alive.
