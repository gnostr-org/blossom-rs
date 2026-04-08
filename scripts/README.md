# scripts/

Utility scripts for development, CI, and infrastructure.

---

## macos-runner.sh

Install and manage a GitHub Actions self-hosted runner on macOS (x86_64/Intel).
Registers against an organization or a single repository with the
`macos-15-intel` label, and installs it as a **launchd service** so it
survives reboots.

### Prerequisites

- macOS on x86_64 (Intel)
- [`gh`](https://cli.github.com/) authenticated with org or repo admin rights
- `curl`, `python3` (both ship with macOS)

---

## linux-runner.sh

Install and manage a GitHub Actions self-hosted runner on Linux (x86_64).
Registers against an organization or a single repository with the
`self-hosted,Linux,X64` labels, and installs it as a **systemd service**.

### Prerequisites

- Linux x86_64
- [`gh`](https://cli.github.com/) authenticated with org or repo admin rights
- `curl`, `python3`
- `sudo` rights (required for systemd service install/uninstall)

---

## Common options

### Usage

```
./scripts/macos-runner.sh [options] [action]
./scripts/linux-runner.sh [options] [action]
```

**Actions**

| Action      | Description                                              |
|-------------|----------------------------------------------------------|
| `install`   | Download, configure, and start the runner *(default)*    |
| `remove`    | Deregister runner from GitHub (keeps files on disk)      |
| `start`     | Start the launchd service                                |
| `stop`      | Stop the launchd service                                 |
| `status`    | Show launchd service status                              |
| `uninstall` | Stop service, deregister from GitHub, delete runner dir  |

**Options**

| Flag | Default | Description |
|------|---------|-------------|
| `--org <org>` | `MonumentalSystems` | GitHub organization |
| `--repo <repo>` | *(none — org-scoped)* | Scope to a single repo; dir becomes `~/actions-runner/<repo>` |
| `--name <name>` | `<hostname>-intel` | Runner display name |
| `--labels <labels>` | `self-hosted,macOS,X64,macos-15-intel` | Comma-separated runner labels |
| `--dir <path>` | `~/actions-runner` or `~/actions-runner/<repo>` | Runner install directory |
| `--group <group>` | `Default` | Runner group |
| `--remove` | — | Remove existing config before installing (reconfigure) |

### Examples

```bash
# macOS — org-scoped runner for MonumentalSystems (default)
./scripts/macos-runner.sh install

# Linux — org-scoped runner for MonumentalSystems (default)
./scripts/linux-runner.sh install

# Org-scoped runner for a different org
./scripts/macos-runner.sh --org gnostr-org install
./scripts/linux-runner.sh --org gnostr-org install

# Repo-scoped runner — isolated dir, repo-level token
./scripts/macos-runner.sh --org gnostr-org --repo blossom-rs install
./scripts/linux-runner.sh --org gnostr-org --repo blossom-rs install

# Multiple repo-scoped runners on the same machine
./scripts/macos-runner.sh --org gnostr-org --repo blossom-rs   install
./scripts/macos-runner.sh --org gnostr-org --repo other-repo   install

# Reconfigure an existing runner (removes old registration first)
./scripts/macos-runner.sh --remove install
./scripts/linux-runner.sh --remove install

# Custom name and labels
./scripts/macos-runner.sh --name my-mac   --labels "self-hosted,macOS,X64,macos-15-intel" install
./scripts/linux-runner.sh --name my-linux --labels "self-hosted,Linux,X64,ubuntu-24" install

# Check status
./scripts/macos-runner.sh --org gnostr-org status
./scripts/linux-runner.sh --org gnostr-org status

# Fully remove
./scripts/macos-runner.sh uninstall
./scripts/linux-runner.sh uninstall
```

### Runner directory layout

```
~/actions-runner/             ← org-scoped runner
~/actions-runner/<repo>/      ← repo-scoped runner (one per repo)
    config.sh
    run.sh
    svc.sh
    _work/                    ← job working directory
```

### Service manager

| OS | Service manager | Requires sudo |
|----|----------------|---------------|
| macOS | launchd (`~/Library/LaunchAgents`) | No |
| Linux | systemd | Yes (install/uninstall only) |

### Environment variables

All flags can also be set via environment variables (flags take precedence):

| Variable | Corresponding flag |
|----------|--------------------|
| `RUNNER_ORG` | `--org` |
| `RUNNER_REPO` | `--repo` |
| `RUNNER_NAME` | `--name` |
| `RUNNER_DIR` | `--dir` |
