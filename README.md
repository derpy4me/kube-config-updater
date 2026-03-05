# kube_config_updater

A CLI tool and interactive TUI for fetching k3s kubeconfig files from remote servers via SSH, caching them locally, and merging them into `~/.kube/config`.

## Features

- **Automatic cert-expiry checking** — skips servers with valid certs; fetches only when expired or unknown
- **OS keyring integration** — passwords stored via the system keyring; file-based fallback (0600 permissions) when no keyring daemon is available
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

#### Linux credential storage

The tool attempts to store SSH passwords in the GNOME Secret Service (D-Bus). On desktop Ubuntu this typically works out of the box. On headless or minimal installs (common for k3s servers), no secret service daemon is running.

**When the keyring is unavailable**, the TUI shows a consent dialog:

```
 Credential Storage Fallback
 ────────────────────────────────────────────────────────────
 Keyring unavailable: Platform secure storage failure: …

 Fallback file: ~/.config/kube_config_updater/credentials
 Permissions:   0600  (only you can read this file)

 This is the same security model used by:
   ~/.kube/config   (kubectl credentials)
   ~/.ssh/id_rsa    (SSH private keys)

 [y] Store to file    [n] Cancel — do not store
```

You must explicitly press **y** to accept file-based storage. Nothing is written until you do. If you press **n**, the server is still added but has no credential; set one later with `c` in the TUI.

**To use the system keyring** (stronger isolation) on a headless server:

```bash
sudo apt-get install gnome-keyring libsecret-1-0
```

Then start a keyring daemon in your session and re-set the credential with `c`.

**Security model of the file fallback**: passwords are stored as base64-encoded text (base64 is encoding, not encryption) in a file with `chmod 0600` — readable only by your Unix user. Root can always read any file. This is identical to how `~/.kube/config`, `~/.ssh/id_rsa`, and `~/.aws/credentials` work.

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

Passwords are stored in the OS keyring when available. On Linux systems without a running secret service daemon, the TUI offers an explicit consent dialog to store credentials in a file with `0600` permissions instead (see [Linux credential storage](#linux-credential-storage) above). Passwords are never stored in the app config file.

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

## Bitwarden / Vaultwarden Integration

Server configs and SSH credentials can optionally be sourced from a Bitwarden or Vaultwarden vault. This enables company-managed access control — admins define which servers each team member can access via Bitwarden organizations and collections.

**This is additive** — the existing keyring + file fallback system remains the default. Vault integration is enabled by adding a `[bitwarden]` section to `config.toml`.

### Setup

1. Install the Bitwarden CLI: `npm install -g @bitwarden/cli`
2. Configure it for your server: `bw config server https://vault.your-company.com`
3. Log in: `bw login`
4. Add the `[bitwarden]` section to your config:

```toml
[bitwarden]
enabled = true
server_url = "https://vault.strata.com"
collection = "K3s Production"
item_prefix = "k3s:"
```

### Vault item format

Each server is a **Login item** in the configured collection:

| Bitwarden field | Maps to | Required |
|---|---|---|
| Item name (after prefix strip) | Server name | yes |
| `login.username` | SSH user | no |
| `login.password` | SSH password | no |
| Custom field `address` | SSH hostname/IP | yes |
| Custom field `target_cluster_ip` | Cluster IP for kubeconfig | yes |
| Custom field `file_path` | Remote file directory | no |
| Custom field `file_name` | Remote file name | no |
| Custom field `context_name` | Kubeconfig context name | no |
| Custom field `identity_file` | SSH private key path | no |

### Server merge

Local `[[server]]` entries in config.toml take precedence over vault items with the same name. This lets you override vault-managed servers locally when needed.

### TUI behavior

- Vault servers show a `[vault]` badge on the dashboard
- Vault servers are read-only — delete and credential management are disabled (managed in Bitwarden)
- The credential column shows "Vault" for vault-sourced servers

---

## Security

### Credential storage

| Method | Where | Security model |
|---|---|---|
| OS keyring (default) | GNOME Secret Service (Linux) / macOS Keychain | Encrypted by the desktop session |
| File fallback | `~/.config/kube_config_updater/credentials` | `chmod 0600` — owner-read-only, base64-encoded (not encrypted) |
| Bitwarden vault | Bitwarden/Vaultwarden server | End-to-end encrypted; decrypted locally by `bw` CLI |

The file fallback uses the same security model as `~/.kube/config`, `~/.ssh/id_rsa`, and `~/.aws/credentials` — readable only by your Unix user. Root can always read any file.

### Vault session handling

- The Bitwarden session key is held **in memory only** — never written to disk by this tool
- The `bw` CLI receives the session key via environment variable (`BW_SESSION`), not CLI argument — this avoids exposure in `ps` output
- Master passwords are cleared from memory immediately after the `bw unlock` call
- Vault passwords (SSH credentials from vault items) are held in a HashMap for the duration of the run, then dropped

### Headless/cron authentication

For unattended operation, the tool supports API key authentication:

- `BW_CLIENTID` and `BW_CLIENTSECRET` environment variables provide login credentials (bypasses 2FA)
- `password_file` in the `[bitwarden]` config section points to a `chmod 600` file containing the master password for vault unlock
- The master password never appears in environment variables or CLI arguments

The tool warns (like SSH does for private keys) if `password_file` has overly permissive file permissions.

### Access control

When Bitwarden is enabled, the tool sees **only servers the authenticated user has access to**. Access control is fully delegated to Bitwarden's organization and collection permissions — the tool does not implement authorization.

---

## Cron usage

The CLI is safe for cron — produces no output when all certs are valid.

```cron
0 6 * * * /usr/local/bin/kube_config_updater --log-dir /var/log/kube_config_updater
```

### With Bitwarden vault

For cron jobs that need vault access, create a wrapper script:

```bash
#!/bin/bash
set -euo pipefail
source /etc/kube-config-updater/bw-credentials   # exports BW_CLIENTID, BW_CLIENTSECRET
/usr/local/bin/kube_config_updater --log-dir /var/log/kube_config_updater
```

The tool handles `bw login --apikey` and `bw unlock --passwordfile` internally when it detects the API key environment variables and a configured `password_file`.

Setup:

```bash
# Store API credentials (from Bitwarden web vault → Settings → Security → Keys)
cat > /etc/kube-config-updater/bw-credentials <<'EOF'
export BW_CLIENTID=user.xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
export BW_CLIENTSECRET=xxxxxxxxxxxxxxxxxxxxxxxxxxxx
EOF
chmod 600 /etc/kube-config-updater/bw-credentials

# Store master password
echo "your-master-password" > /etc/kube-config-updater/bw-password
chmod 600 /etc/kube-config-updater/bw-password
```

Then add `password_file` to your config:

```toml
[bitwarden]
enabled = true
server_url = "https://vault.strata.com"
collection = "K3s Production"
item_prefix = "k3s:"
password_file = "/etc/kube-config-updater/bw-password"
```

---

## Project structure

```
src/
├── main.rs           CLI entry point and command routing
├── bitwarden.rs      Bitwarden/Vaultwarden CLI wrapper and vault item parsing
├── fetch.rs          Server processing loop (parallel, cert-skip logic)
├── config.rs         Config loading, server add/remove (comment-preserving)
├── credentials.rs    OS keyring + file fallback credential storage
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
        ├── bitwarden.rs   Vault unlock prompt (render + key handler)
        ├── credentials.rs     Credential set/delete UI
        ├── keyring_fallback.rs Consent dialog for file-based credential fallback
        └── help.rs            Help modal
```
