# Linux Computer Use

Linux Computer Use is an opt-in UI surface backed by a native Rust MCP backend,
`codex-computer-use-linux`. The backend is bundled and registered by default;
the in-app Computer Use controls are disabled until you opt in.

After rebuilding or installing an update, fully quit every running Codex window
and reopen the app once. The launcher refreshes bundled plugin caches and tool
registration only during a cold start; opening another window on an older
running process cannot activate a newly installed backend.

It supports:

- app listing and accessibility trees through AT-SPI
- screenshots through GNOME Shell DBus, the Codex GNOME Shell extension, or XDG Desktop Portal
- window listing and focusing on GNOME, KWin/Plasma, Hyprland, Niri, COSMIC,
  i3, and EWMH-compliant X11 desktops including Cinnamon, MATE, and XFCE
- keyboard, text, click, scroll, and drag input through `/dev/uinput`, XDG
  RemoteDesktop portal, or `ydotool`
- full-Unicode X11 text entry through a transactional native clipboard path;
  plain-text clipboard contents are restored and `xdotool` sends only the
  paste chord (`Ctrl+Shift+V` for recognized terminals)
- continuous multi-point drawing gestures for handwriting, curves, and lassos
- exact-focus, window-relative drags and drawing paths that reject any
  out-of-bounds endpoint or point before sending pointer input
- exact X11 window activation plus EWMH move/resize control on Cinnamon, MATE,
  XFCE, and compatible window managers
- application-focused X11 discovery that omits desktop, dock, notification,
  splash, and taskbar/pager-hidden utility surfaces

Window move/resize behavior is backend-specific:

- KWin applies requested frame geometry and verifies the resulting bounds
- Hyprland dispatches exact pixel geometry and verifies the result
- i3 applies exact geometry only when the current layout/container permits it,
  which normally means a floating window
- the COSMIC helper currently has no window-geometry protocol, so move/resize
  remains unavailable there

## Runtime Dependencies

On X11, install `xdotool` for reliable full-Unicode text entry. It sends a
modifier-clean paste chord while the Rust backend temporarily owns the X11
clipboard selection; it is not used to store or retrieve the text itself:

```bash
# Debian / Ubuntu
sudo apt install xdotool

# Fedora
sudo dnf install xdotool

# Arch / Manjaro
sudo pacman -S xdotool

# openSUSE
sudo zypper install xdotool
```

The deb and RPM packages recommend `xdotool`, and the Arch package lists it as
an optional dependency. Every Nix output places `xdotool` on the app launcher
`PATH` because the Computer Use backend is available in every package variant.

If `xdotool` is absent, X11 text entry falls back safely to the regular
keyboard backend before changing the clipboard. Install `ydotool` when you
need that global fallback input path for keyboard and pointer actions:

The transaction compares and claims clipboard ownership atomically, restores
only while it still owns the temporary selection, and preserves a newer user
copy. Because the current owner implementation can faithfully restore one
plain-text payload but not HTML, images, URI lists, or application-specific
formats, mixed/rich clipboards are deliberately left untouched. Non-ASCII
`type_text` then returns an explicit error instead of silently degrading the
clipboard; `set_value` remains available for an exposed editable AT-SPI field.

Install `ydotool` 1.0 or newer when you need the fallback input path. Some
Debian and Ubuntu releases still package the incompatible pre-1.0 CLI; the
Computer Use readiness report detects and rejects it instead of sending unsafe
input commands.

```bash
# Debian / Ubuntu
sudo apt install ydotool
sudo apt install ydotoold   # on Ubuntu releases that split the daemon

# Fedora
sudo dnf install ydotool

# Arch / Manjaro
sudo pacman -S ydotool

# openSUSE
sudo zypper install ydotool
```

The preferred coordinate input path opens `/dev/uinput` directly; that device
is pointer-only in this backend and does not by itself make keyboard input
ready. The XDG RemoteDesktop portal can provide both on desktops that expose
it. Targeted keyboard actions pin the resolved window and reverify it
immediately before delivery.

For `ydotool`, run a daemon and make sure your user can access the socket:

```bash
sudo systemctl enable --now ydotoold
sudo usermod -a -G input "$USER"
```

Then log out and back in.

Some distros name the unit `ydotool.service` instead of `ydotoold.service`, and
some install `/usr/bin/ydotoold` without a service unit. If the system unit path
is awkward, a user-session service that binds `%t/.ydotool_socket` is also
valid.

Portal packages are needed when your desktop relies on XDG Desktop Portal input
or screenshots:

- KDE Plasma: `xdg-desktop-portal-kde`
- sway/wlroots: `xdg-desktop-portal-wlr`
- Hyprland: `xdg-desktop-portal-hyprland`
- GNOME: usually available by default

Niri window listing and exact focus use the `niri` command and the active
session's `NIRI_SOCKET`. The Computer Use backend hydrates `NIRI_SOCKET` for GUI
starts, but the socket must still belong to the active Niri session and be
reachable by the desktop user.

## Verify Readiness

Once Computer Use is visible in the Codex UI, ask Codex:

> Check whether Linux Computer Use is ready

You can also run the backend directly:

```bash
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux doctor
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux doctor --summary
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux setup
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux apps
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux windows
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux screenshot
```

The guided setup can discover an installed, staged, cached, or `PATH` backend,
run the doctor with a bounded timeout, and reduce its JSON report to a
ready/degraded checklist:

```bash
CODEX_BOOTSTRAP_RUN_COMPUTER_USE_DOCTOR=1 make setup-native
```

This verification is read-only. It does not run the mutating `setup` or
`setup-window-targeting` commands, install packages, start services, or change
device/group permissions. A `READY` result requires accessibility, window
querying, exact focus, input, and a screenshot backend; `DEGRADED` includes the
detected blockers and recommended next step.

## Enable The In-App UI

Ad hoc, for one build:

```bash
CODEX_LINUX_ENABLE_COMPUTER_USE_UI=1 make build-app
```

Persistent, including future auto-updater rebuilds:

```bash
CODEX_BOOTSTRAP_COMPUTER_USE_UI=1 make setup-native
```

This validates and atomically merges the flag into the app's `settings.json`,
preserving unrelated settings. It refuses to overwrite malformed JSON. To opt
back out persistently:

```bash
CODEX_BOOTSTRAP_COMPUTER_USE_UI=0 make setup-native
```

Unset the ad-hoc build environment variable as well if you used it. Persistent
UI changes apply after rebuilding/reinstalling and do not bypass unrelated
upstream server-side availability.

Nix:

```bash
nix run github:alingold/codex-desktop-linux#codex-desktop-computer-use-ui
```

Combined with a Linux feature output:

```bash
nix run github:alingold/codex-desktop-linux#computer-use-ui-remote-mobile-control
```

## Side-By-Side Dev Variant

```bash
make build-dev-app
make run-dev-app
```

Override the dev identity with `DEV_APP_ID`, `DEV_APP_NAME`, and
`CODEX_WEBVIEW_PORT` if needed.
