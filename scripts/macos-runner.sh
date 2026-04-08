#!/usr/bin/env bash
# macos-runner.sh — Install and configure a GitHub Actions self-hosted runner
# with the macos-15-intel label.
#
# Prerequisites:
#   - gh CLI authenticated with an account that has org admin rights
#   - macOS x86_64 (Intel)
#
# Usage:
#   ./scripts/macos-runner.sh [options] [install|start|stop|status|uninstall]
#
# Options:
#   --org <org>          GitHub organization (default: MonumentalSystems)
#   --name <name>        Runner name (default: <hostname>-intel)
#   --labels <labels>    Comma-separated runner labels
#                        (default: self-hosted,macOS,X64,macos-15-intel)
#   --dir <path>         Runner install directory (default: ~/actions-runner)
#   --group <group>      Runner group (default: Default)
#
# Examples:
#   ./scripts/macos-runner.sh --org my-org install
#   ./scripts/macos-runner.sh --org my-org --name my-runner status

set -euo pipefail

RUNNER_DIR="${RUNNER_DIR:-$HOME/actions-runner}"
ORG="${RUNNER_ORG:-MonumentalSystems}"
RUNNER_NAME="${RUNNER_NAME:-$(hostname -s)-intel}"
RUNNER_LABELS="self-hosted,macOS,X64,macos-15-intel"
RUNNER_GROUP="Default"

# ── helpers ──────────────────────────────────────────────────────────────────
info()  { echo "[info]  $*"; }
error() { echo "[error] $*" >&2; exit 1; }

usage() {
    awk '/^# Usage:/,/^[^#]/' "$0" | grep '^#' | sed 's/^#[[:space:]]\{0,2\}//'
    exit 0
}

require() {
    command -v "$1" &>/dev/null || error "'$1' is required but not found"
}

# ── argument parsing ──────────────────────────────────────────────────────────
ACTION="install"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --org)    ORG="${2:?'--org requires a value'}";              shift 2 ;;
        --name)   RUNNER_NAME="${2:?'--name requires a value'}";     shift 2 ;;
        --labels) RUNNER_LABELS="${2:?'--labels requires a value'}"; shift 2 ;;
        --dir)    RUNNER_DIR="${2:?'--dir requires a value'}";       shift 2 ;;
        --group)  RUNNER_GROUP="${2:?'--group requires a value'}";   shift 2 ;;
        --help|-h) usage ;;
        install|start|stop|status|uninstall) ACTION="$1"; shift ;;
        *) error "Unknown argument '$1'. Run with --help for usage." ;;
    esac
done

latest_runner_url() {
    curl -fsSL https://api.github.com/repos/actions/runner/releases/latest \
        | python3 -c "
import sys, json
assets = json.load(sys.stdin)['assets']
url = next(a['browser_download_url'] for a in assets
           if 'osx-x64' in a['name'] and a['name'].endswith('.tar.gz'))
print(url)
"
}

# ── commands ─────────────────────────────────────────────────────────────────
cmd_install() {
    require gh
    require curl
    require python3

    info "Fetching registration token for org: $ORG"
    TOKEN=$(gh api "orgs/${ORG}/actions/runners/registration-token" \
                --method POST --jq '.token')

    info "Resolving latest runner release..."
    URL=$(latest_runner_url)
    VERSION=$(echo "$URL" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
    info "Runner version: $VERSION"

    info "Installing runner to $RUNNER_DIR"
    mkdir -p "$RUNNER_DIR"

    ARCHIVE="$RUNNER_DIR/runner.tar.gz"
    curl -fsSL "$URL" -o "$ARCHIVE"
    tar xzf "$ARCHIVE" -C "$RUNNER_DIR"
    rm "$ARCHIVE"

    info "Configuring runner (name=$RUNNER_NAME, labels=$RUNNER_LABELS)"
    "$RUNNER_DIR/config.sh" \
        --url "https://github.com/${ORG}" \
        --token "$TOKEN" \
        --name "$RUNNER_NAME" \
        --labels "$RUNNER_LABELS" \
        --runnergroup "$RUNNER_GROUP" \
        --work "$RUNNER_DIR/_work" \
        --unattended \
        --replace

    info "Installing as a launchd service (runs at login)"
    "$RUNNER_DIR/svc.sh" install
    "$RUNNER_DIR/svc.sh" start

    info "Done. Runner '$RUNNER_NAME' is registered and running."
    info "View at: https://github.com/organizations/${ORG}/settings/actions/runners"
}

cmd_start() {
    info "Starting runner service"
    "$RUNNER_DIR/svc.sh" start
}

cmd_stop() {
    info "Stopping runner service"
    "$RUNNER_DIR/svc.sh" stop
}

cmd_status() {
    "$RUNNER_DIR/svc.sh" status
}

cmd_uninstall() {
    require gh
    info "Stopping and removing service"
    "$RUNNER_DIR/svc.sh" stop  || true
    "$RUNNER_DIR/svc.sh" uninstall || true

    info "Fetching removal token for org: $ORG"
    TOKEN=$(gh api "orgs/${ORG}/actions/runners/remove-token" \
                --method POST --jq '.token')

    info "Removing runner from GitHub"
    "$RUNNER_DIR/config.sh" remove --token "$TOKEN"

    info "Deleting $RUNNER_DIR"
    rm -rf "$RUNNER_DIR"
    info "Runner uninstalled."
}

# ── entrypoint ────────────────────────────────────────────────────────────────
case "$ACTION" in
    install)   cmd_install   ;;
    start)     cmd_start     ;;
    stop)      cmd_stop      ;;
    status)    cmd_status    ;;
    uninstall) cmd_uninstall ;;
    *)         error "Unknown action '$ACTION'. Use: install | start | stop | status | uninstall" ;;
esac
