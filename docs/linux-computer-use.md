# Linux Computer Use

Linux Computer Use is an opt-in UI surface backed by a native Rust MCP backend,
`codex-computer-use-linux`. The backend is bundled and registered by default;
the in-app Computer Use controls are disabled until you opt in.

It supports:

- app listing and accessibility trees through AT-SPI
- screenshots through GNOME Shell DBus, the Codex GNOME Shell extension, or XDG Desktop Portal
- window listing and focusing on GNOME, KWin/Plasma, Hyprland, COSMIC, i3,
  and EWMH-compliant X11 desktops including Cinnamon, MATE, and XFCE
- keyboard, click, scroll, and drag input through `/dev/uinput`, XDG
  RemoteDesktop portal, or `ydotool`
- full-Unicode X11 text entry through a transactional native clipboard path;
  the previous clipboard is restored and `xdotool` sends only the paste chord
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
an optional dependency. Nix Computer Use UI outputs place `xdotool` on the app
launcher `PATH`; the base Nix output does not add it.

If `xdotool` is absent, X11 text entry falls back safely to the regular
keyboard backend before changing the clipboard. Install `ydotool` when you
need that global fallback input path for keyboard and pointer actions:

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

The preferred coordinate input path opens `/dev/uinput` directly. The XDG
RemoteDesktop portal can also provide input on desktops that expose it.

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

## Verify Readiness

Once Computer Use is visible in the Codex UI, ask Codex:

> Check whether Linux Computer Use is ready

You can also run the backend directly:

```bash
./codex-app/resources/plugins/openai-bundled/plugins/computer-use/bin/codex-computer-use-linux doctor
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
nix run github:ilysenko/codex-desktop-linux#codex-desktop-computer-use-ui
```

Combined with a Linux feature output:

```bash
nix run github:ilysenko/codex-desktop-linux#computer-use-ui-remote-mobile-control
```

## Side-By-Side Dev Variant

```bash
make build-dev-app
make run-dev-app
```

Override the dev identity with `DEV_APP_ID`, `DEV_APP_NAME`, and
`CODEX_WEBVIEW_PORT` if needed.
