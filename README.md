# kube_config_updater

A CLI tool and interactive TUI for fetching k3s kubeconfig files from remote servers via SSH, caching them locally, and merging them into `~/.kube/config`.

## Features

- **Automatic cert-expiry checking** — skips servers with valid certs; fetches only when expired or unknown
- **OS keyring integration** — passwords stored securely via the system keyring (never in config files)
- **Parallel processing** — all servers fetched concurrently with a progress bar
- **Interactive TUI** — dashboard with server list, cert expiry, fetch status, and per-server detail view
- **Server cert probe** — read-only SSH check to compare remote cert against local cache without writing
- **Fetch delta notifications** — shows whether a cert was renewed, unchanged, or still expired after fetch
- **Add-server wizard** — guided 8-step wizard with live connection test before saving
- **Dry-run mode** — preview all actions without writing files

---

## Installation

### Pre-built binary (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/derpy4me/kube-config-updater/main/install.sh | sh
```

Installs to `/usr/local/bin/kube_config_updater` (or `~/.local/bin` if `/usr/local/bin` isn't writable).

To pin a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/derpy4me/kube-config-updater/main/install.sh | sh -s -- v0.2.0
```

Or download a binary directly from [GitHub Releases](https://github.com/derpy4me/kube-config-updater/releases).

#### Supported platforms

| OS | Architecture | Binary |
|---|---|---|
| Linux | x86_64 | `kube_config_updater-linux-x86_64` |
| macOS | Apple Silicon (M1/M2/M3/M4) | `kube_config_updater-macos-arm64` |

#### macOS note

The binary is ad-hoc signed but not notarized (no Apple Developer Program required). The install script removes the quarantine attribute automatically. If you download manually and Gatekeeper blocks it:

```bash
xattr -d com.apple.quarantine /usr/local/bin/kube_config_updater
```

#### Linux runtime requirement

The keyring integration requires `libsecret` at runtime (GNOME secret service). On Ubuntu:

```bash
sudo apt-get install libsecret-1-0
```

A running secret service daemon is also required (`gnome-keyring-daemon` on desktop Ubuntu). Headless / server installs without a secret service are not currently supported — the tool will exit with an error rather than fall back to plaintext storage.

### Build from source

```bash
cargo build --release
# Binary at: target/release/kube_config_updater
```

---

## Configuration

Default config path: `~/.kube_config_updater/config.toml`

```toml
# Defaults applied to all servers (can be overridden per server)
default_user = "ubuntu"
default_file_path = "/etc/rancher/k3s"
default_file_name = "k3s.yaml"
# default_identity_file = "~/.ssh/id_ed25519"

# Directory where fetched kubeconfigs are written
local_output_dir = "/home/user/.kube"

[[server]]
name = "prod-k3s"
address = "10.0.1.10"
target_cluster_ip = "10.0.1.10"
context_name = "prod"

[[server]]
name = "staging-k3s"
address = "10.0.2.10"
target_cluster_ip = "10.0.2.10"
user = "admin"
context_name = "staging"
```

### Config fields

| Field | Required | Description |
|---|---|---|
| `local_output_dir` | yes | Directory for cached per-server kubeconfigs |
| `default_user` | no | SSH user if not set per server |
| `default_file_path` | no | Remote file directory if not set per server |
| `default_file_name` | no | Remote file name if not set per server |
| `default_identity_file` | no | SSH private key path if not set per server |

### Server fields (`[[server]]`)

| Field | Required | Description |
|---|---|---|
| `name` | yes | Unique identifier; used for local file name and credential lookup |
| `address` | yes | SSH hostname or IP |
| `target_cluster_ip` | yes | IP written into the fetched kubeconfig's cluster URL |
| `context_name` | no | Context name in the merged `~/.kube/config` (defaults to `name`) |
| `user` | no | SSH user (overrides `default_user`) |
| `file_path` | no | Remote directory (overrides `default_file_path`) |
| `file_name` | no | Remote file name (overrides `default_file_name`) |
| `identity_file` | no | SSH private key path (overrides `default_identity_file`) |

---

## Usage

### Fetch all servers (CLI)

```bash
kube_config_updater
```

Skips servers with valid certs. Use `--dry-run` to preview without writing.

```bash
kube_config_updater --dry-run
kube_config_updater --servers prod-k3s staging-k3s
kube_config_updater --log-dir /var/log/kube_config_updater
```

### Interactive TUI

```bash
kube_config_updater tui
```

#### Dashboard keys

| Key | Action |
|---|---|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` / `G` | First / last |
| `Enter` | Open detail view |
| `f` | Force fetch selected server |
| `F` | Force fetch all servers |
| `a` | Add server (wizard) |
| `D` | Delete selected server |
| `c` | Manage credentials |
| `d` | Toggle dry-run mode |
| `e` | Edit config in `$EDITOR` |
| `?` | Help |
| `q` / `Ctrl+C` / `Ctrl+D` | Quit |

#### Detail view keys

| Key | Action |
|---|---|
| `f` | Force fetch |
| `p` | Probe remote cert (read-only SSH check) |
| `c` | Manage credentials |
| `Esc` / `q` | Back |

### Manage credentials

Passwords are stored in the OS keyring, never in config files.

```bash
# Store a password (prompts securely)
kube_config_updater credential set --server prod-k3s

# Store a shared default (used when no server-specific credential exists)
kube_config_updater credential set --default

# Remove a credential
kube_config_updater credential delete --server prod-k3s

# List which servers have stored credentials
kube_config_updater credential list
```

Credentials can also be set/deleted from within the TUI using `c`.

---

## How it works

For each server, the tool:

1. **Checks local cert expiry** — reads `~/.kube/<server_name>` and inspects the cached `preferences.certificate-expires-at` field. Skips fetch if cert is still valid (unless `--force` / `f` in TUI).
2. **Looks up credentials** — checks keyring for server-specific credential, falls back to `_default`.
3. **SSH fetches the remote kubeconfig** — authenticates with identity file, password, or SSH agent (in that priority order). Uses `sudo -S cat` for password-based access.
4. **Writes the local file** — saves raw content to `<local_output_dir>/<server_name>`.
5. **Processes the kubeconfig** — rewrites the cluster URL to `https://<target_cluster_ip>:6443`, renames cluster/context/user entries to `<context_name>` for conflict-free merging, and embeds cert expiry + source hash in `preferences`.
6. **Merges into `~/.kube/config`** — upserts cluster, context, and user entries; never modifies `current-context` or other entries.

---

## State file

Run status is persisted to `/tmp/kube_config_updater_state.json` and read by the TUI. The TUI watches for changes and refreshes automatically.

```json
{
  "prod-k3s": {
    "status": "Fetched",
    "last_updated": "2025-02-20T15:30:45Z",
    "error": null
  }
}
```

Status values: `Fetched` · `Skipped` · `NoCredential` · `AuthRejected` · `Failed`

---

## Cron usage

The CLI is safe for cron — produces no output when all certs are valid.

```cron
0 6 * * * /usr/local/bin/kube_config_updater --log-dir /var/log/kube_config_updater
```

---

## Project structure

```
src/
├── main.rs           CLI entry point and command routing
├── fetch.rs          Server processing loop (parallel, cert-skip logic)
├── config.rs         Config loading, server add/remove (comment-preserving)
├── credentials.rs    OS keyring integration
├── state.rs          Run state persistence (JSON, atomic writes)
├── ssh.rs            SSH connection and remote file retrieval
├── kube.rs           Kubeconfig parsing, cert extraction, merge logic
└── tui/
    ├── mod.rs         Event loop, render/key dispatch, spawn_fetch
    ├── app.rs         App state types (View, WizardState, ProbeState, …)
    └── features/      Vertical slice: each module owns render + key handler
        ├── mod.rs         Shared UI utilities (colors, layout helpers)
        ├── dashboard.rs   Server list, delete confirm, error overlay
        ├── detail.rs      Server detail, cert probe
        ├── wizard.rs      Add-server wizard, connection test
        ├── credentials.rs Credential set/delete UI
        └── help.rs        Help modal
```
