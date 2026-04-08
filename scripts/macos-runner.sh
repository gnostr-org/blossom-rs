#!/usr/bin/env bash
# macos-runner.sh — Install and configure a GitHub Actions self-hosted runner
# for the gnostr-org organization with the macos-15-intel label.
#
# Prerequisites:
#   - gh CLI authenticated with an account that has org admin rights
#   - macOS x86_64 (Intel)
#
# Usage:
#   ./scripts/macos-runner.sh [install|start|stop|uninstall]
#   Default action: install

set -euo pipefail

RUNNER_DIR="${RUNNER_DIR:-$HOME/actions-runner}"
ORG="gnostr-org"
RUNNER_NAME="${RUNNER_NAME:-$(hostname -s)-intel}"
RUNNER_LABELS="self-hosted,macOS,X64,macos-15-intel"
RUNNER_GROUP="Default"

# ── helpers ──────────────────────────────────────────────────────────────────
info()  { echo "[info]  $*"; }
error() { echo "[error] $*" >&2; exit 1; }

require() {
    command -v "$1" &>/dev/null || error "'$1' is required but not found"
}

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
ACTION="${1:-install}"
case "$ACTION" in
    install)   cmd_install   ;;
    start)     cmd_start     ;;
    stop)      cmd_stop      ;;
    status)    cmd_status    ;;
    uninstall) cmd_uninstall ;;
    *)         error "Unknown action '$ACTION'. Use: install | start | stop | status | uninstall" ;;
esac
