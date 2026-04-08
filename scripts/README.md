# scripts/

Utility scripts for development, CI, and infrastructure.

---

## macos-runner.sh

Install and manage a GitHub Actions self-hosted runner on macOS.
**Auto-detects architecture at runtime** — no flags needed to distinguish
Intel vs Apple Silicon. Installs as a **launchd service** so it survives reboots.

| Arch | Default labels | Runner binary |
|------|---------------|---------------|
| x86_64 (Intel) | `self-hosted,macOS,X64,macos-15-intel` | `osx-x64` |
| arm64 (Apple Silicon) | `self-hosted,macOS,ARM64,macos-latest` | `osx-arm64` |

### Prerequisites

- macOS (Intel or Apple Silicon)
- [`gh`](https://cli.github.com/) authenticated with org or repo admin rights
- `curl`, `python3` (both ship with macOS)

---

## linux-runner.sh

Install and manage a GitHub Actions self-hosted runner on Linux (x86_64).
Registers with labels `self-hosted,Linux,X64` and installs as a **systemd service**.

### Prerequisites

- Linux x86_64
- [`gh`](https://cli.github.com/) authenticated with org or repo admin rights
- `curl`, `python3`
- `sudo` rights (required for systemd service install/uninstall)

---

## windows-runner.ps1

Install and manage a GitHub Actions self-hosted runner on Windows (x86_64).
Registers with labels `self-hosted,Windows,X64,windows-latest` and installs
as a **Windows service** via the runner's built-in `svc.cmd`.

### Prerequisites

- Windows x86_64
- [`gh`](https://cli.github.com/) authenticated with org or repo admin rights
- PowerShell 7+ (or Windows PowerShell 5.1 with `??` operator support)
- Run as **Administrator** (required for Windows service install/uninstall)

### Usage

```powershell
.\scripts\windows-runner.ps1 [[-Action] <action>] [options]
```

**Actions**

| Action      | Description                                              |
|-------------|----------------------------------------------------------|
| `install`   | Download, configure, and start the runner *(default)*    |
| `remove`    | Deregister runner from GitHub (keeps files on disk)      |
| `start`     | Start the service                                        |
| `stop`      | Stop the service                                         |
| `status`    | Show service status                                      |
| `uninstall` | Stop service, deregister from GitHub, delete runner dir  |

**Options**

| Parameter | Default | Description |
|-----------|---------|-------------|
| `-Org` | `MonumentalSystems` | GitHub organization |
| `-Repo` | *(none — org-scoped)* | Scope to a single repo |
| `-Name` | `<COMPUTERNAME>-windows` | Runner display name |
| `-Labels` | `self-hosted,Windows,X64,windows-latest` | Comma-separated labels |
| `-Dir` | `$Home\actions-runner` | Runner install directory |
| `-Group` | `Default` | Runner group |
| `-Remove` | — | Remove existing config before installing |

### Examples

```powershell
# Default install
.\scripts\windows-runner.ps1 install

# Different org
.\scripts\windows-runner.ps1 -Org gnostr-org install

# Repo-scoped runner
.\scripts\windows-runner.ps1 -Org gnostr-org -Repo blossom-rs install

# Reconfigure existing runner
.\scripts\windows-runner.ps1 -Remove install

# Status / teardown
.\scripts\windows-runner.ps1 status
.\scripts\windows-runner.ps1 uninstall
```

### Archive caching

Same strategy as the bash scripts: when `-Repo` is used, `runner.zip` is
cached in the parent dir (`$Home\actions-runner\runner.zip`) and copied into
the repo-scoped dir for sibling repos.

```powershell
# Force re-download
Remove-Item "$HOME\actions-runner\runner.zip"
.\scripts\windows-runner.ps1 -Repo blossom-rs install
```

---



### Usage

```
./scripts/macos-runner.sh [options] [action]
./scripts/linux-runner.sh [options] [action]
.\scripts\windows-runner.ps1 [[-Action] <action>] [options]
```

**Actions**

| Action      | Description                                              |
|-------------|----------------------------------------------------------|
| `install`   | Download, configure, and start the runner *(default)*    |
| `remove`    | Deregister runner from GitHub (keeps files on disk)      |
| `start`     | Start the service                                        |
| `stop`      | Stop the service                                         |
| `status`    | Show service status                                      |
| `uninstall` | Stop service, deregister from GitHub, delete runner dir  |

**Options**

| Flag | Default | Description |
|------|---------|-------------|
| `--org <org>` | `MonumentalSystems` | GitHub organization |
| `--repo <repo>` | *(none — org-scoped)* | Scope to a single repo; dir becomes `~/actions-runner/<repo>` |
| `--name <name>` | `<hostname>-intel` or `<hostname>-arm` | Runner display name (auto-set by arch on macOS) |
| `--labels <labels>` | *(auto by arch on macOS)* | Comma-separated runner labels |
| `--dir <path>` | `~/actions-runner` or `~/actions-runner/<repo>` | Runner install directory |
| `--group <group>` | `Default` | Runner group |
| `--remove` | — | Remove existing config before installing (reconfigure) |

### Tar caching

When `--repo` is used, runner tarballs are cached in the **parent directory**
(`~/actions-runner/runner.tar.gz`) and copied into the repo-scoped dir, so
sibling repos skip the download entirely.

To upgrade the cached version, delete the parent tar and re-run:

```bash
rm ~/actions-runner/runner.tar.gz
./scripts/macos-runner.sh --repo blossom-rs install
```

### Examples

```bash
# macOS — auto-detects Intel or ARM, registers accordingly
./scripts/macos-runner.sh install

# Linux
./scripts/linux-runner.sh install

# Different org
./scripts/macos-runner.sh --org gnostr-org install
./scripts/linux-runner.sh --org gnostr-org install

# Repo-scoped runner — isolated dir, repo-level token
./scripts/macos-runner.sh --org gnostr-org --repo blossom-rs install
./scripts/linux-runner.sh --org gnostr-org --repo blossom-rs install

# Multiple repo-scoped runners on the same machine (tar downloaded once)
./scripts/macos-runner.sh --org gnostr-org --repo blossom-rs install
./scripts/macos-runner.sh --org gnostr-org --repo other-repo  install

# Reconfigure an existing runner
./scripts/macos-runner.sh --remove install
./scripts/linux-runner.sh --remove install

# Custom labels
./scripts/macos-runner.sh --labels "self-hosted,macOS,ARM64,macos-latest" install
./scripts/linux-runner.sh --labels "self-hosted,Linux,X64,ubuntu-24" install

# Status / teardown
./scripts/macos-runner.sh status
./scripts/macos-runner.sh uninstall
```

### Runner directory layout

```
~/actions-runner/
    runner.tar.gz             ← cached archive (org-scoped, or shared by --repo installs)
    config.sh                 ← org-scoped runner config
    _work/
    <repo>/                   ← repo-scoped runner (one per repo)
        runner.tar.gz         ← copy of parent archive
        config.sh
        _work/
```

### Service manager

| OS | Service manager | Requires elevated rights |
|----|----------------|--------------------------|
| macOS | launchd (`~/Library/LaunchAgents`) | No |
| Linux | systemd | Yes — `sudo` (install/uninstall only) |
| Windows | Windows Service (`svc.cmd`) | Yes — run as Administrator |

### Environment variables

All flags can also be set via environment variables (flags take precedence):

| Variable | Corresponding flag |
|----------|--------------------|
| `RUNNER_ORG` | `--org` |
| `RUNNER_REPO` | `--repo` |
| `RUNNER_NAME` | `--name` |
| `RUNNER_DIR` | `--dir` |
