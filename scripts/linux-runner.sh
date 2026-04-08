#!/usr/bin/env bash
# linux-runner.sh — Install and configure a GitHub Actions self-hosted runner
# on Linux (x86_64).
#
# Prerequisites:
#   - gh CLI authenticated with an account that has org or repo admin rights
#   - Linux x86_64
#   - curl, python3
#   - sudo rights (required for systemd service install/uninstall)
#
# Usage:
#   ./scripts/linux-runner.sh [options] [install|start|stop|status|uninstall|remove]
#
# Options:
#   --org <org>          GitHub organization (default: MonumentalSystems)
#   --repo <repo>        Scope runner to a single repo instead of the org.
#                        Runner dir defaults to ~/actions-runner/<repo>.
#                        Uses repo-level token API (requires repo admin rights).
#   --name <name>        Runner name (default: <hostname>-linux)
#   --labels <labels>    Comma-separated runner labels
#                        (default: self-hosted,Linux,X64)
#   --dir <path>         Runner install directory
#                        (default: ~/actions-runner or ~/actions-runner/<repo>)
#   --group <group>      Runner group (default: Default)
#   --remove             Remove existing runner config before installing
#
# Examples:
#   ./scripts/linux-runner.sh install
#   ./scripts/linux-runner.sh --org my-org install
#   ./scripts/linux-runner.sh --org my-org --name my-linux status
#   ./scripts/linux-runner.sh --remove install             # reconfigure from scratch
#   ./scripts/linux-runner.sh --repo blossom-rs install    # repo-scoped runner

set -euo pipefail

RUNNER_DIR=""   # resolved after arg parsing
ORG="${RUNNER_ORG:-MonumentalSystems}"
REPO="${RUNNER_REPO:-}"
RUNNER_NAME="${RUNNER_NAME:-$(hostname -s)-linux}"
RUNNER_LABELS="self-hosted,Linux,X64"
RUNNER_GROUP="Default"
FORCE_REMOVE=0

# ── helpers ──────────────────────────────────────────────────────────────────
info()  { echo "[info]  $*"; }
error() { echo "[error] $*" >&2; exit 1; }

# Run a command in the background with an ASCII spinner on the right.
# Usage: with_spinner "label" cmd [args...]
with_spinner() {
    local label="$1"; shift
    local log; log="$(mktemp)"
    "$@" >"$log" 2>&1 &
    local pid=$!
    local frames=('|' '/' '-' '\\')
    local i=0
    while kill -0 "$pid" 2>/dev/null; do
        printf "\r[info]  %s  %s " "$label" "${frames[$((i % 4))]}"
        i=$((i + 1))
        sleep 0.1
    done
    wait "$pid"
    local rc=$?
    if [[ $rc -eq 0 ]]; then
        printf "\r[info]  %s  ✓\n" "$label"
    else
        printf "\r[error] %s  ✗ (exit %d)\n" "$label" "$rc" >&2
        cat "$log" >&2
        rm -f "$log"
        exit "$rc"
    fi
    rm -f "$log"
}

usage() {
    awk '/^# Usage:/,/^[^#]/' "$0" | grep '^#' | sed 's/^#[[:space:]]\{0,2\}//'
    exit 0
}

require() {
    command -v "$1" &>/dev/null || error "'$1' is required but not found"
}

# systemd service install/uninstall requires sudo on Linux
svc() {
    local needs_sudo=0
    case "$1" in install|uninstall) needs_sudo=1 ;; esac
    if [[ $needs_sudo -eq 1 ]]; then
        (cd "$RUNNER_DIR" && sudo ./svc.sh "$@")
    else
        (cd "$RUNNER_DIR" && ./svc.sh "$@")
    fi
}

# ── argument parsing ──────────────────────────────────────────────────────────
ACTION="install"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --org)    ORG="${2:?'--org requires a value'}";              shift 2 ;;
        --repo)   REPO="${2:?'--repo requires a value'}";            shift 2 ;;
        --name)   RUNNER_NAME="${2:?'--name requires a value'}";     shift 2 ;;
        --labels) RUNNER_LABELS="${2:?'--labels requires a value'}"; shift 2 ;;
        --dir)    RUNNER_DIR="${2:?'--dir requires a value'}";       shift 2 ;;
        --group)  RUNNER_GROUP="${2:?'--group requires a value'}";   shift 2 ;;
        --remove) FORCE_REMOVE=1; shift ;;
        --help|-h) usage ;;
        install|start|stop|status|uninstall|remove) ACTION="$1"; shift ;;
        *) error "Unknown argument '$1'. Run with --help for usage." ;;
    esac
done

# Resolve RUNNER_DIR after all args are parsed
if [[ -z "$RUNNER_DIR" ]]; then
    if [[ -n "$REPO" ]]; then
        RUNNER_DIR="$HOME/actions-runner/$REPO"
    else
        RUNNER_DIR="${RUNNER_DIR_DEFAULT:-$HOME/actions-runner}"
    fi
fi

# ── token helpers (org vs repo scoped) ───────────────────────────────────────
registration_token() {
    if [[ -n "$REPO" ]]; then
        gh api "repos/${ORG}/${REPO}/actions/runners/registration-token" \
            --method POST --jq '.token'
    else
        gh api "orgs/${ORG}/actions/runners/registration-token" \
            --method POST --jq '.token'
    fi
}

removal_token() {
    if [[ -n "$REPO" ]]; then
        gh api "repos/${ORG}/${REPO}/actions/runners/remove-token" \
            --method POST --jq '.token'
    else
        gh api "orgs/${ORG}/actions/runners/remove-token" \
            --method POST --jq '.token'
    fi
}

runner_url() {
    if [[ -n "$REPO" ]]; then
        echo "https://github.com/${ORG}/${REPO}"
    else
        echo "https://github.com/${ORG}"
    fi
}

runners_settings_url() {
    if [[ -n "$REPO" ]]; then
        echo "https://github.com/${ORG}/${REPO}/settings/actions/runners"
    else
        echo "https://github.com/organizations/${ORG}/settings/actions/runners"
    fi
}

scope_label() {
    if [[ -n "$REPO" ]]; then echo "repo: $ORG/$REPO"; else echo "org: $ORG"; fi
}

latest_runner_url() {
    curl -fsSL https://api.github.com/repos/actions/runner/releases/latest \
        | python3 -c "
import sys, json
assets = json.load(sys.stdin)['assets']
url = next(a['browser_download_url'] for a in assets
           if 'linux-x64' in a['name'] and a['name'].endswith('.tar.gz'))
print(url)
"
}

# ── commands ─────────────────────────────────────────────────────────────────
cmd_remove() {
    if [[ ! -f "$RUNNER_DIR/config.sh" ]]; then
        info "No runner config found at $RUNNER_DIR — nothing to remove."
        return 0
    fi
    require gh
    info "Fetching removal token ($(scope_label))"
    TOKEN=$(removal_token)
    info "Removing runner config from GitHub"
    "$RUNNER_DIR/config.sh" remove --token "$TOKEN"
}

cmd_install() {
    require gh
    require curl
    require python3

    if [[ $FORCE_REMOVE -eq 1 ]]; then
        cmd_remove
    fi

    info "Fetching registration token ($(scope_label))"
    TOKEN=$(registration_token)

    info "Resolving latest runner release..."
    URL=$(latest_runner_url)
    VERSION=$(echo "$URL" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
    info "Runner version: $VERSION"

    info "Installing runner to $RUNNER_DIR"
    mkdir -p "$RUNNER_DIR"

    ARCHIVE="$RUNNER_DIR/runner.tar.gz"
    PARENT_ARCHIVE="$(dirname "$RUNNER_DIR")/runner.tar.gz"

    if [[ -n "$REPO" && -f "$PARENT_ARCHIVE" ]]; then
        info "Reusing existing archive from parent: $PARENT_ARCHIVE"
        echo "[warn]  If the runner version is stale, delete the cached archive and re-run:"
        echo "[warn]    rm $PARENT_ARCHIVE"
        cp "$PARENT_ARCHIVE" "$ARCHIVE"
    else
        info "Downloading runner archive..."
        # Download to parent dir when repo-scoped so sibling repos can reuse it
        if [[ -n "$REPO" ]]; then
            curl -#fL "$URL" -o "$PARENT_ARCHIVE"
            cp "$PARENT_ARCHIVE" "$ARCHIVE"
        else
            curl -#fL "$URL" -o "$ARCHIVE"
        fi
    fi
    with_spinner "Extracting runner archive..." tar xzf "$ARCHIVE" -C "$RUNNER_DIR"

    info "Configuring runner (name=$RUNNER_NAME, labels=$RUNNER_LABELS)"
    with_spinner "Configuring runner..." \
        "$RUNNER_DIR/config.sh" \
            --url "$(runner_url)" \
            --token "$TOKEN" \
            --name "$RUNNER_NAME" \
            --labels "$RUNNER_LABELS" \
            --runnergroup "$RUNNER_GROUP" \
            --work "$RUNNER_DIR/_work" \
            --unattended \
            --replace

    info "Installing as a systemd service (requires sudo)"
    with_spinner "Installing systemd service..." svc install
    with_spinner "Starting service..."            svc start

    info "Done. Runner '$RUNNER_NAME' is registered and running."
    info "View at: $(runners_settings_url)"
}

cmd_start() {
    info "Starting runner service"
    svc start
}

cmd_stop() {
    info "Stopping runner service"
    svc stop
}

cmd_status() {
    svc status
}

cmd_uninstall() {
    require gh
    info "Stopping and removing systemd service (requires sudo)"
    svc stop      || true
    svc uninstall || true

    info "Fetching removal token ($(scope_label))"
    TOKEN=$(removal_token)

    info "Removing runner from GitHub"
    "$RUNNER_DIR/config.sh" remove --token "$TOKEN"

    info "Deleting $RUNNER_DIR"
    rm -rf "$RUNNER_DIR"
    info "Runner uninstalled."
}

# ── entrypoint ────────────────────────────────────────────────────────────────
case "$ACTION" in
    install)   cmd_install   ;;
    remove)    cmd_remove    ;;
    start)     cmd_start     ;;
    stop)      cmd_stop      ;;
    status)    cmd_status    ;;
    uninstall) cmd_uninstall ;;
    *)         error "Unknown action '$ACTION'. Use: install | remove | start | stop | status | uninstall" ;;
esac
