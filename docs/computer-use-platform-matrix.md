# Linux Computer Use Platform Matrix

Linux Computer Use support is reported in evidence tiers. A backend being
present in the source does not by itself mean that every distro/session
combination is certified.

## Evidence tiers

| Tier | Meaning |
| --- | --- |
| Live certified | Exercised end-to-end in a real user desktop session, including screenshots and input. |
| Runtime tested | Exercised against real windows in an isolated compositor/window-manager session. |
| Build tested | Compiled and unit-tested on the distro family in CI. |
| Implemented | Backend and unit tests exist, but the session still needs live certification. |

## Current desktop coverage

| Session/backend | Evidence | Window list/focus | Move/resize | Input | Screenshot | Accessibility |
| --- | --- | --- | --- | --- | --- | --- |
| Linux Mint Cinnamon / X11 | Live certified | EWMH | EWMH | absolute uinput, portal, ydotool | GNOME-compatible DBus or portal | native Rust AT-SPI |
| Generic EWMH X11 / Metacity | Runtime tested | EWMH | EWMH | safety validation only in headless CI | not exercised in headless CI | not exercised in headless CI |
| GNOME / Wayland or X11 | Implemented | Shell extension or Introspect | Shell extension | portal, absolute uinput, ydotool | Shell, extension, or portal | native Rust AT-SPI |
| KDE Plasma / Wayland or X11 | Implemented | KWin scripting | KWin scripting where supported | portal, absolute uinput, ydotool | portal | native Rust AT-SPI |
| Hyprland | Implemented | `hyprctl` | `hyprctl` where supported | portal, absolute uinput, ydotool | portal | native Rust AT-SPI |
| i3 | Implemented | `i3-msg` | `i3-msg` where supported | absolute uinput or ydotool | portal or compatible screenshot backend | native Rust AT-SPI |
| COSMIC | Implemented | bundled Wayland helper | backend-dependent | portal, absolute uinput, ydotool | portal | native Rust AT-SPI |

## Current distro coverage

The Computer Use crate has a GitHub Actions build/test matrix for Ubuntu 24.04,
Debian 12, Fedora, Arch Linux, and openSUSE Tumbleweed. Packaging smoke tests
cover deb, rpm, pacman, AppImage, and Nix paths. These checks validate build and
packaging portability; they do not substitute for a live desktop-session test.

## Required release gates

Before calling a desktop/session live certified:

1. `doctor` reports window, screenshot, input, and AT-SPI readiness accurately.
2. A real app can be listed, exactly focused, and targeted by window id.
3. Window-cropped screenshot coordinates round-trip to relative click and path input.
4. Literal text covers ASCII, punctuation, multiline text, and non-ASCII Unicode.
5. Move/resize either works and reports compositor-final geometry or returns an explicit unsupported result.
6. Multi-monitor and fractional-scale coordinates are verified when the session supports them.
7. The setup flow starts from a clean user account and leaves a reversible record of changes.

## Known cross-platform limitations

- Sandboxed Snap/Flatpak applications can restrict AT-SPI or desktop integration.
- Portal permission behavior varies by desktop and portal implementation.
- Some X11 clients omit `_NET_WM_PID`; exact window id, title, and class targeting remain available.
- Electron applications may require `--force-renderer-accessibility` before their UI tree is exposed.
- A tool schema is fixed for an active Codex task; installing a backend with new tools requires a new task or app restart before those tools appear.
