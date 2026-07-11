# Native Setup

This project has two native install entrypoints:

- `make bootstrap-native` for the fastest non-interactive first install.
- `make setup-native` for a guided checklist and optional Linux feature picker.

## Fast Native Install

```bash
git clone https://github.com/ilysenko/codex-desktop-linux.git
cd codex-desktop-linux
make bootstrap-native
```

`make bootstrap-native` installs build dependencies, regenerates `codex-app/`,
validates the cached upstream `Codex.dmg` and downloads it only when missing or
stale, builds the matching native package, and installs the newest artifact
from `dist/`.

If dependencies are already installed:

```bash
make install-native
```

## Guided Setup

```bash
make setup-native
```

The wizard detects your distro, package manager, native package format, desktop
session, GUI prompt helpers, `pkexec`, portal status, installed package state,
updater state, Computer Use UI/readiness state, and optional Linux feature
manifests.

It can write the git-ignored `linux-features/features.json` file for the next
build. You can choose features by id, number, or range in the prompt. It can
also persistently opt the in-app Computer Use surface in or out without
clobbering unrelated keys in `settings.json`.

The wizard is intentionally separate from `make bootstrap-native`,
`make install-native`, `make package`, and `make install`, which stay
non-interactive for scripts and CI.

## Non-Interactive Feature Selection

```bash
CODEX_LINUX_FEATURES=remote-mobile-control,read-aloud \
CODEX_LINUX_DISABLE_FEATURES=conversation-mode \
PACKAGE_WITH_UPDATER=0 \
CODEX_BOOTSTRAP_NONINTERACTIVE=1 \
make setup-native
```

Enable the opt-in Computer Use UI and run its read-only readiness check:

```bash
CODEX_BOOTSTRAP_COMPUTER_USE_UI=1 \
CODEX_BOOTSTRAP_RUN_COMPUTER_USE_DOCTOR=1 \
CODEX_BOOTSTRAP_NONINTERACTIVE=1 \
make setup-native
```

Use `CODEX_BOOTSTRAP_COMPUTER_USE_UI=0` to opt back out. The writer validates
the existing JSON, preserves every unrelated setting, and replaces the file
atomically. It refuses to overwrite malformed or non-object JSON. UI changes
apply after the next build/reinstall and do not bypass unrelated upstream
server-side availability.

To have the wizard orchestrate existing install commands, opt in explicitly:

```bash
CODEX_BOOTSTRAP_DRY_RUN=1 \
CODEX_BOOTSTRAP_INSTALL_DEPS=1 \
CODEX_BOOTSTRAP_INSTALL_NATIVE=1 \
make setup-native
```

```bash
CODEX_BOOTSTRAP_INSTALL_DEPS=1 \
CODEX_BOOTSTRAP_INSTALL_NATIVE=1 \
make setup-native
```

Build-time feature changes only apply after rebuilding and reinstalling:

```bash
make install-native
```

For manual-update packages:

```bash
PACKAGE_WITH_UPDATER=0 make install-native
```

## Computer Use Verification

When the backend is installed or staged, guided setup offers to run its
read-only doctor. After a native install requested through the wizard, the
doctor runs automatically unless explicitly disabled:

```bash
CODEX_BOOTSTRAP_RUN_COMPUTER_USE_DOCTOR=0 make setup-native
```

The summary reports `READY` only when MCP registration, accessibility, window
querying, exact focus, input injection, and screenshots are all available.
Otherwise it reports `DEGRADED`, lists blockers, and prints the backend's
recommended next step. The check never enables accessibility, installs
packages, starts services, or changes group membership.

The default timeout is 15 seconds. It can be adjusted from 1 to 300 seconds:

```bash
CODEX_BOOTSTRAP_DOCTOR_TIMEOUT=30 \
CODEX_BOOTSTRAP_RUN_COMPUTER_USE_DOCTOR=1 \
make setup-native
```

For development or side-by-side builds, select an exact backend explicitly:

```bash
CODEX_BOOTSTRAP_COMPUTER_USE_BIN=/path/to/codex-computer-use-linux \
CODEX_BOOTSTRAP_RUN_COMPUTER_USE_DOCTOR=1 \
make setup-native
```

## Feature Cleanup

Disabling a feature in `features.json` affects the next rebuild. It does not
delete local device keys, Read Aloud model files, plugin caches, Python
runtimes, or services.

Feature cleanup is separate and interactive:

```bash
CODEX_BOOTSTRAP_CLEANUP_FEATURES=remote-mobile-control,read-aloud make setup-native
```

Each deletion requires typing `DELETE <exact path>`. Preview cleanup targets:

```bash
CODEX_BOOTSTRAP_DRY_RUN=1 \
CODEX_BOOTSTRAP_CLEANUP_FEATURES=remote-mobile-control,read-aloud \
make setup-native
```

## Color Output

The wizard uses ANSI color when the terminal supports it.

```bash
CODEX_BOOTSTRAP_COLOR=0 make setup-native  # disable
CODEX_BOOTSTRAP_COLOR=1 make setup-native  # force
```
