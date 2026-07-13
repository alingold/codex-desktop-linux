#!/bin/bash
# Install-time detection of an already-running ChatGPT Desktop instance.
#
# Sourced by install.sh. Do not run directly.
# shellcheck shell=bash

canonical_path() {
    realpath -m "$1"
}

pid_is_current_user() {
    local pid="$1"
    local uid

    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [ -d "/proc/$pid" ] || return 1
    uid="$(awk '/^Uid:/ {print $2}' "/proc/$pid/status" 2>/dev/null || true)"
    [ "$uid" = "$(id -u)" ]
}

# Electron helper processes (renderer, gpu-process, utility, zygote, ...)
# all carry their role as a `--type=...` argv entry. Only the main app
# process omits it, so we use this to skip orphaned helpers that survive
# their parent and re-attach to systemd.
cmdline_has_electron_helper_type() {
    local cmdline_path="$1"
    [ -r "$cmdline_path" ] || return 1
    tr '\000' '\n' < "$cmdline_path" 2>/dev/null | grep -q '^--type=' && return 0
    LC_ALL=C grep -a -q -- ' --type=' "$cmdline_path" 2>/dev/null
}

pid_is_electron_helper() {
    local pid="$1"
    cmdline_has_electron_helper_type "/proc/$pid/cmdline"
}

pid_cmdline_arg0_path() {
    local pid="$1"
    local actual=""

    [ -r "/proc/$pid/cmdline" ] || return 1
    IFS= read -r -d '' actual < "/proc/$pid/cmdline" || [ -n "$actual" ] || return 1
    [ -n "$actual" ] || return 1
    canonical_path "$actual"
}

pid_matches_install_target() {
    local pid="$1"
    local expected="$2"
    local actual

    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [ -d "/proc/$pid" ] || return 1
    pid_is_current_user "$pid" || return 1
    # Match argv[0], not /proc/<pid>/exe. A package manager can atomically
    # replace Electron while the old process remains alive; its procfs
    # executable then ends in " (deleted)", but argv[0] still identifies the
    # install it came from.
    actual="$(pid_cmdline_arg0_path "$pid" 2>/dev/null || true)"
    [ -n "$actual" ] || return 1
    [ "$actual" = "$(canonical_path "$expected")" ] || return 1
    ! pid_is_electron_helper "$pid"
}

warn_if_running_install_requires_restart() {
    local pid=""

    if pid="$(find_running_install_target_pid)"; then
        printf '%s\n' \
            "[WARN] $CODEX_APP_DISPLAY_NAME is still running from $INSTALL_DIR (pid $pid)." \
            "[WARN] That process predates the package just installed. Fully quit it, then reopen the app so bundled plugins and tools are registered from the new build." >&2
    fi
}

find_running_install_target_pid() {
    local electron_path="$INSTALL_DIR/electron"
    local app_pid_file="${XDG_STATE_HOME:-$HOME/.local/state}/$CODEX_APP_ID/app.pid"
    local pid
    local proc_exe

    [ -e "$electron_path" ] || return 1

    if [ -f "$app_pid_file" ]; then
        pid="$(cat "$app_pid_file" 2>/dev/null || true)"
        if pid_matches_install_target "$pid" "$electron_path"; then
            echo "$pid"
            return 0
        fi
    fi

    for proc_exe in /proc/[0-9]*/exe; do
        [ -e "$proc_exe" ] || continue
        pid="${proc_exe#/proc/}"
        pid="${pid%/exe}"
        if pid_matches_install_target "$pid" "$electron_path"; then
            echo "$pid"
            return 0
        fi
    done

    return 1
}

assert_install_target_not_running() {
    if ! install_target_is_stopped; then
        error "ChatGPT Desktop is currently running from $INSTALL_DIR (pid $RUNNING_INSTALL_TARGET_PID).
Close that app before rebuilding this install directory, or build into a separate path:
  CODEX_INSTALL_DIR=/tmp/codex-app-build ./install.sh

Set CODEX_INSTALL_ALLOW_RUNNING=1 only if you intentionally want to overwrite a running app."
    fi
}

install_target_is_stopped() {
    local pid
    RUNNING_INSTALL_TARGET_PID=""

    if [ "${CODEX_INSTALL_ALLOW_RUNNING:-0}" = "1" ]; then
        warn "CODEX_INSTALL_ALLOW_RUNNING=1 set; installer may overwrite a running ChatGPT app"
        return 0
    fi

    if pid="$(find_running_install_target_pid)"; then
        RUNNING_INSTALL_TARGET_PID="$pid"
        export RUNNING_INSTALL_TARGET_PID
        return 1
    fi

    return 0
}
