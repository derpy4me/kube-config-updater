# Workflow: kube_config_updater Core Completion

**Updated**: 2026-02-19
**Based on**: Two `/sc:brainstorm` sessions — cert renewal logic + security/credential management

---

## Overview

Complete the core functionality of `kube_config_updater` so it is production-ready and
safe to run from cron multiple times per day. This requires three interlocking concerns:

1. **Credential security** — store SSH passwords in the OS keyring (never on disk), add
   SSH password auth, and use `sudo -S` on remotes where needed
2. **Cert expiry gating** — only SSH out when a cert is actually expired; skip silently otherwise
3. **Kubeconfig merging** — after a fetch, upsert the new context into `~/.kube/config`

These are sequenced as: Phase 0 (security) → Phase 1 (cert check) → Phase 2 (merge) →
Phase 3 (cron output) → Phase 4 (tests) → Phase 5 (improve) → Phase 6 (cleanup).

**Phase 0 is a prerequisite**: it changes `fetch_remote_file`'s signature, which Phase 1
calls. All subsequent phases depend on Phase 0 being complete.

---

## Phase 0 — Security: Credential Management

**Goal**: Remove plaintext credentials from disk. Store SSH/sudo passwords in the OS
keyring. Add password-based SSH auth and `sudo -S` remote execution. Introduce the
`credential` subcommand for managing stored passwords.

### 0.1 — Add Dependencies to `Cargo.toml`

```toml
keyring  = "3"       # cross-platform OS keyring (libsecret/GNOME on Linux, Keychain on macOS)
rpassword = "7"      # hidden terminal password input for credential set
```

### 0.2 — Remove Plaintext Password from Config

**File**: `kube_config.toml`, line 2

Delete the line:
```toml
default_password = "L0serw1nner!"
```

Replace it with a comment explaining how credentials are now managed:
```toml
# Passwords are stored securely in the OS keyring.
# To set a credential: kube_config_updater credential set --server <name>
# To set a default credential: kube_config_updater credential set --default
```

The `Config` struct never had a `default_password` field — serde silently ignored it.
No struct changes needed.

### 0.3 — Create `src/credentials.rs`

New module. Constants and types:

```rust
const SERVICE: &str = "kube_config_updater";
const DEFAULT_ACCOUNT: &str = "_default";

pub enum CredentialResult {
    /// A credential was found; contains the password
    Found(String),
    /// No credential stored for this server (not an error — use key/agent instead)
    NotFound,
    /// Keyring is locked or the service is unavailable (e.g. pre-login cron run)
    Unavailable(String),  // String carries the underlying error message for logging
}
```

Public functions:

```rust
/// Look up a credential for the given server name.
/// Falls back to the "_default" account if no server-specific entry exists.
pub fn get_credential(server_name: &str) -> CredentialResult

/// Store (or update) a credential for the given server name.
/// Pass "_default" as server_name to set the default credential.
pub fn set_credential(server_name: &str, password: &str) -> Result<(), anyhow::Error>

/// Remove a credential for the given server name.
pub fn delete_credential(server_name: &str) -> Result<(), anyhow::Error>

/// For each server name in the slice, return whether a credential exists.
/// Used by `credential list` — loads config to enumerate server names.
pub fn check_credentials(server_names: &[&str]) -> Vec<(String, bool)>
```

**`get_credential` logic**:
1. Try `keyring::Entry::new(SERVICE, server_name)?.get_password()`
2. If `Ok(pw)` → return `Found(pw)`
3. If `NoEntry` error → try `keyring::Entry::new(SERVICE, DEFAULT_ACCOUNT)?.get_password()`
4. If `Ok(pw)` → return `Found(pw)` (default credential)
5. If `NoEntry` → return `NotFound`
6. If any other keyring error (e.g. `NoDbus`, `Locked`) → return `Unavailable(err.to_string())`

**Security invariants**:
- Passwords must **never** appear in log output at any level
- Passwords must **never** be stored in any struct field that implements `Debug` (use
  a newtype wrapper or `#[serde(skip)]` if ever serialized)

### 0.4 — Add `credential` Subcommand to CLI in `main.rs`

Add to the existing `Cli` struct:

```rust
#[command(subcommand)]
command: Option<Commands>,
```

Add the enum:

```rust
#[derive(Subcommand)]
enum Commands {
    /// Manage SSH credentials stored in the OS keyring
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
}

#[derive(Subcommand)]
enum CredentialAction {
    /// Store a credential for a server (prompts for password if --password is omitted)
    Set {
        #[arg(long, group = "target")]
        server: Option<String>,
        #[arg(long, group = "target")]
        default: bool,
        #[arg(long)]
        password: Option<String>,
    },
    /// Remove a stored credential
    Delete {
        #[arg(long, group = "target")]
        server: Option<String>,
        #[arg(long, group = "target")]
        default: bool,
    },
    /// Show which servers have a stored credential (never shows passwords)
    List,
}
```

In `main()`, dispatch before entering `process_servers`:

```rust
if let Some(Commands::Credential { action }) = cli.command {
    // handle credential actions, then return Ok(())
    // credential set: if --password omitted, use rpassword::prompt_password()
    // credential list: load config, call check_credentials with server names
}
```

**`credential set` behavior**:
- If `--password` is provided: store directly
- If `--password` is omitted: call `rpassword::prompt_password("Password: ")` — hidden input
- Account name: server name (from `--server`) or `DEFAULT_ACCOUNT` (from `--default`)
- Print confirmation to stdout: `"Credential stored for server 'k3s-sandbox'."`

**`credential list` behavior**:
- Load the config file (uses same config_path logic as main run)
- For each server in config, call `check_credentials`
- Print a table to stdout — server name + `[SET]` or `[NOT SET]`, never the password value
- Also check and show whether `_default` is set

### 0.5 — Update `fetch_remote_file` in `src/ssh.rs`

**New signature**:

```rust
pub fn fetch_remote_file(
    server_name: &str,
    server_address: &str,
    user: &str,
    remote_path: &str,
    identity_file: Option<&str>,
    password: Option<&str>,         // NEW: SSH login + sudo password; None = use key/agent
) -> Result<Vec<u8>, anyhow::Error>
```

**Updated auth logic** (replace the existing `if let Some(key_path)` block):

```rust
if let Some(key_path) = identity_file {
    // Key-based auth (existing behavior)
    session.userauth_pubkey_file(user, None, Path::new(key_path), None)?;
} else if let Some(pw) = password {
    // Password auth (new)
    session.userauth_password(user, pw)?;
} else {
    // SSH agent fallback (existing behavior)
    session.userauth_agent(user)?;
}
```

**Updated remote command** (replace the `cat` command construction):

```rust
let (command, use_sudo) = if password.is_some() {
    (format!("sudo -S cat {}", remote_path), true)
} else {
    (format!("cat {}", remote_path), false)
};

let mut channel = session.channel_session()?;
channel.exec(&command)?;

if use_sudo {
    // Write the password to stdin so sudo -S can read it
    // The sudo prompt ("password:") goes to stderr — handled below
    use std::io::Write;
    channel.write_all(format!("{}\n", password.unwrap()).as_bytes())?;
}
```

**Stderr handling update**: `sudo -S` writes its prompt to stderr. The existing code
already reads and logs stderr. No changes needed — a non-zero exit code still means
failure; the sudo prompt on stderr is harmless.

**Note**: The password is written to the channel's stdin (not passed as a command
argument), so it never appears in the remote process list or audit logs.

---

## Phase 1 — Cert Expiry Decision Layer

**Goal**: Before opening any SSH connection, check whether the local cached kubeconfig
has a valid cert. Only fetch if the cert is expired, missing, or unreadable. Also gate
on credential availability — skip with a warning if the keyring is locked.

**Depends on**: Phase 0 complete (credential lookup + updated `fetch_remote_file` signature).

### 1.1 — Add `CertStatus` enum to `kube.rs`

```rust
pub enum CertStatus {
    /// Cert is still valid — no action needed
    Valid(chrono::DateTime<chrono::Utc>),
    /// Cert is expired — fetch needed
    Expired,
    /// No local file, missing field, or parse error — treat as unknown, fetch to be safe
    Unknown,
}
```

### 1.2 — Add `check_local_cert_expiry(path: &Path) -> CertStatus` to `kube.rs`

New public function. Logic:

1. If `path` does not exist → return `CertStatus::Unknown`
2. Read and parse the YAML → on error return `CertStatus::Unknown`
3. Look up `preferences["certificate-expires-at"]` → if missing return `CertStatus::Unknown`
4. Parse the RFC3339 string into `DateTime<Utc>` → on error return `CertStatus::Unknown`
5. If `expiry <= Utc::now()` → return `CertStatus::Expired`
6. Otherwise → return `CertStatus::Valid(expiry)`

No new dependencies needed (`chrono` and `serde_yaml` are already present).

### 1.3 — Add `SkipReason` and `ServerResult` enums to `main.rs`

```rust
enum SkipReason {
    CertValid(chrono::DateTime<chrono::Utc>),
    KeyringUnavailable,
}

enum ServerResult {
    Fetched,
    Skipped(SkipReason),
}
```

### 1.4 — Rewrite `process_server` in `main.rs`

Change return type to `Result<ServerResult, anyhow::Error>`.

Full updated logic flow:

```
1. Resolve user, remote_path_str, identity_file from server + config
2. Build local_path: PathBuf::from(&config.local_output_dir) / &server.name

3. CHECK CERT EXPIRY:
   match check_local_cert_expiry(&local_path) {
       CertStatus::Valid(expiry) =>
           log::debug!("[{}] Cert valid until {}, skipping", server.name, expiry);
           return Ok(ServerResult::Skipped(SkipReason::CertValid(expiry)))
       CertStatus::Expired =>
           log::info!("[{}] Cert expired, fetching...", server.name)
       CertStatus::Unknown =>
           log::info!("[{}] Cert status unknown (no cache), fetching...", server.name)
   }

4. LOOKUP CREDENTIAL:
   let password: Option<String> = match get_credential(&server.name) {
       CredentialResult::Found(pw) => Some(pw),
       CredentialResult::NotFound   => None,
       CredentialResult::Unavailable(reason) => {
           log::warn!("[{}] Keyring unavailable ({}). Skipping. \
                       Run 'credential set' or log in to unlock keyring.",
                      server.name, reason);
           return Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable))
       }
   };

5. FETCH:
   let contents = fetch_remote_file(
       &server.name,
       &server.address,
       user,
       &remote_path_str,
       identity_file,
       password.as_deref(),   // NEW parameter
   )?;

6. HASH:
   let source_hash = compute_sha256(&contents);

7. WRITE LOCAL FILE (if not dry_run):
   fs::write(&local_path, contents)?;
   log::info!("[{}] Config written to {:?}", server.name, local_path);

8. PROCESS KUBECONFIG:
   process_kubeconfig_file(&local_path, &server.target_cluster_ip,
                           &source_hash, &server.context_name, dry_run)?;

9. MERGE INTO MAIN CONFIG (Phase 2):
   merge_into_main_kubeconfig(&local_path, dry_run)?;

10. return Ok(ServerResult::Fetched)
```

**Dry-run**: cert check and credential lookup still run; only file writes are suppressed.
If the cert is valid, dry-run still skips (consistent with normal run).

---

## Phase 2 — Merge into `~/.kube/config`

**Goal**: After a successful fetch, upsert the new cluster/context/user entries into
`~/.kube/config`. Always replace entries with the same name.

**Depends on**: Phase 1 structure complete (`process_server` calling this function).

### 2.1 — Add `merge_into_main_kubeconfig` to `kube.rs`

```rust
pub fn merge_into_main_kubeconfig(
    fetched_path: &Path,
    dry_run: bool,
) -> Result<(), anyhow::Error>
```

Logic:

1. Read and parse `fetched_path` (the per-server file after `process_kubeconfig_file` ran)
2. Determine `~/.kube/config` path via `dirs::home_dir()` — error if unavailable
3. If `~/.kube/config` exists → read and parse it; if not → create an empty `KubeConfig`
   (`api_version: "v1"`, `kind: "Config"`, `current_context: ""`, empty vecs)
4. For each `ClusterInfo` in the fetched config: remove any existing entry in the target
   with the same `.name`, then push the new entry
5. Repeat for `ContextInfo` and `UserInfo`
6. Do **not** copy `preferences` — those are per-server metadata fields (cert expiry,
   source hash, timestamps) and do not belong in the shared config
7. Do **not** change `current_context` of the main config
8. If `dry_run` → log what would be merged, do not write
9. If not `dry_run` → serialize to YAML and write to `~/.kube/config`
10. Log: `"[{}] Merged cluster/context/user into ~/.kube/config"` with server name

### 2.2 — Wire `merge_into_main_kubeconfig` into `process_server`

This is step 9 of the Phase 1.4 flow — already shown there. No separate wiring needed.

> **Implementation note**: Phase 2.1 defines `merge_into_main_kubeconfig`. Phase 1.4
> calls it. Therefore **Phase 2.1 must be implemented before Phase 1.4** even though
> they are documented in separate phases. The execution order section reflects this.
> You may stub the function with `todo!()` during Phase 1.4 if you need a temporary
> placeholder, but the full implementation must be in place before running tests.

---

## Phase 3 — Cron-Friendly Output

**Goal**: Zero stdout noise when everything is healthy. Actionable warnings when
the keyring is unavailable. Meaningful log summary when fetches occurred.

**Depends on**: Phase 1 (`SkipReason` and `ServerResult` enums defined).

### 3.1 — Update `process_servers` in `main.rs`

Change the results loop to track all three outcomes:

```rust
let mut fetched: u32 = 0;
let mut skipped_cert_valid: u32 = 0;
let mut skipped_no_cred: u32 = 0;
let mut failed: u32 = 0;

for (server, result) in results {
    match result {
        Ok(ServerResult::Fetched) => {
            fetched += 1;
            log::info!("[{}] Successfully fetched and merged.", server.name);
        }
        Ok(ServerResult::Skipped(SkipReason::CertValid(_))) => {
            skipped_cert_valid += 1;
            // Intentionally silent — this is the expected happy path
        }
        Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable)) => {
            skipped_no_cred += 1;
            // Warning already logged inside process_server; just count here
        }
        Err(e) => {
            failed += 1;
            log::error!("[{}] FAILED: {}", server.name, e);
        }
    }
}

// Only emit a summary if something notable happened
if fetched > 0 || failed > 0 || skipped_no_cred > 0 {
    log::info!(
        "Done. fetched={} skipped_cert_valid={} skipped_no_cred={} failed={}",
        fetched, skipped_cert_valid, skipped_no_cred, failed
    );
}
// If only skipped_cert_valid > 0: total silence — ideal for cron
```

### 3.2 — Progress Bar Suppression

The `indicatif` progress bar currently renders unconditionally. This produces output
even when every server is skipped. Update `process_servers` to suppress it:

- If all servers will be skipped (cannot know in advance without pre-checking),
  use `bar.finish_and_clear()` instead of `bar.finish_with_message(...)` when
  `fetched == 0 && failed == 0 && skipped_no_cred == 0`.
- This ensures the progress bar leaves no trace in clean runs.

---

## Phase 4 — Tests

**Goal**: Cover all new logic. Distinguish between unit-testable logic and
integration tests that require real OS services.

### 4.1 — Tests for `check_local_cert_expiry` (unit tests, `src/tests.rs`)

All use `tempfile` (already a dev-dep) to create kubeconfig YAML with controlled
`certificate-expires-at` values in `preferences`.

| Test name | Scenario | Expected |
|---|---|---|
| `test_cert_expiry_no_file` | Path doesn't exist | `Unknown` |
| `test_cert_expiry_no_field` | Valid YAML, no `certificate-expires-at` | `Unknown` |
| `test_cert_expiry_expired` | `certificate-expires-at` set to 1970-01-01 | `Expired` |
| `test_cert_expiry_valid` | `certificate-expires-at` set to 2099-01-01 | `Valid(_)` |
| `test_cert_expiry_bad_date` | `certificate-expires-at` is not a valid date | `Unknown` |

### 4.2 — Tests for `merge_into_main_kubeconfig` (unit tests, `src/tests.rs`)

All use temp directories — never touch the real `~/.kube/config`.

| Test name | Scenario | Expected |
|---|---|---|
| `test_merge_into_empty_config` | No existing target config | Creates it with cluster/context/user |
| `test_merge_replaces_existing` | Target has same-named context | Entry is replaced |
| `test_merge_preserves_other_contexts` | Target has other contexts | Those are untouched |
| `test_merge_dry_run` | `dry_run = true` | Target file is not written |
| `test_merge_no_preferences_copied` | Source has `preferences` metadata | Target gains none |
| `test_merge_preserves_current_context` | Target has `current_context` set | Not changed |

### 4.3 — Tests for `credentials.rs` (unit tests where possible)

The keyring itself is OS-dependent and cannot be reliably unit tested. Test the
logic around it instead:

| Test name | Type | Scenario |
|---|---|---|
| `test_get_credential_not_found_returns_not_found` | Unit (mock) | Entry absent → `NotFound` |
| `test_get_credential_falls_back_to_default` | Integration | Server entry absent, default present → `Found` |
| `test_credential_set_and_delete` | Integration | Set → verify Found; Delete → verify NotFound |

**For unit tests**: abstract keyring access behind a `KeyringBackend` trait in
`credentials.rs`. In tests, inject a `HashMap`-backed mock. In production, use the
real `keyring::Entry` implementation.

```rust
// In credentials.rs
pub trait KeyringBackend {
    fn get(&self, service: &str, account: &str) -> Result<String, keyring::Error>;
    fn set(&self, service: &str, account: &str, password: &str) -> Result<(), keyring::Error>;
    fn delete(&self, service: &str, account: &str) -> Result<(), keyring::Error>;
}
```

This trait exists **only for testability** — production code uses `keyring::Entry`
directly via a concrete implementation of the trait.

### 4.4 — Tests for `process_server` flow (integration test, `src/tests.rs`)

`process_server` orchestrates cert check → credential lookup → fetch → write → merge.
It cannot be meaningfully unit-tested without mocking SSH connections. Document as a
structured integration test:

| Test name | Scenario | Expected |
|---|---|---|
| `test_process_server_skips_valid_cert` | Local file has future `certificate-expires-at` | Returns `Skipped(CertValid)`, no SSH called |
| `test_process_server_skips_keyring_unavailable` | Keyring returns `Unavailable` | Returns `Skipped(KeyringUnavailable)`, no SSH called |

For `test_process_server_skips_valid_cert` and `test_process_server_skips_keyring_unavailable`:
these CAN be unit tested because they return before any SSH call. Use `tempfile` for the
local kubeconfig (cert check) and inject a mock credential backend (keyring check).
The SSH portion is untestable without a real server — that gap is acceptable.

### 4.5 — SSH sudo behavior (integration test note)

The `sudo -S` stdin write behavior cannot be tested without a real SSH server.
Document this as a manual integration test:

```
# Manual integration test:
# 1. Set a credential: kube_config_updater credential set --server <name>
# 2. Run: kube_config_updater --servers <name> --dry-run
# 3. Verify: logs show "Authentication successful" using password auth
# 4. Verify: remote command executed as sudo (check remote server's auth.log)
```

---

## Phase 5 — `/sc:improve` Pass

**Goal**: Code quality improvements on all new and modified code.

Target files and specific review points:

**`src/credentials.rs`** (new):
- Does `CredentialResult::Unavailable(String)` carry enough context for the log message?
- Is the `KeyringBackend` trait clean and minimal?
- Is `DEFAULT_ACCOUNT` the right name / value?

**`src/ssh.rs`** (modified):
- Is the `sudo -S` stdin write safe? Could `channel.write_all` block if the remote
  sudo prompt doesn't appear? (It should not — the channel's stdin is non-blocking
  from the SSH perspective, but verify.)
- Are the auth-method log messages consistent with other log messages in the file?
- Is the `use std::io::Write` import scoped appropriately?

**`src/kube.rs`** (new functions):
- `check_local_cert_expiry`: any `unwrap()` that should be `?`?
- `merge_into_main_kubeconfig`: is the empty KubeConfig initialization clean?
- Are log messages consistent with the `[server_name]` prefix pattern?

**`src/main.rs`** (restructured):
- Is `process_server` getting too long? Consider splitting credential lookup into a
  separate function `resolve_auth(server, config) -> Result<AuthInfo, ...>`.
- Is the progress bar suppression logic clear?
- Is the `Commands` dispatch in `main()` clean and not duplicating config loading?

**`src/config.rs`**:
- No changes expected, but confirm no password-related fields crept in.

---

## Phase 6 — `/sc:cleanup` Pass

**Goal**: Remove dead code and ensure the full codebase is coherent after all phases.

Target the entire `src/` directory plus `Cargo.toml` and `kube_config.toml`:

1. Confirm `default_password` is removed from `kube_config.toml` (Phase 0.2)
2. Confirm `Config` struct has no `password` field
3. Verify all unused imports are removed (especially any `env_logger` vs `flexi_logger`
   confusion — both are in Cargo.toml but only one should be used)
4. Check `add_cert_expiration` in `kube.rs`: it parses the cert and stores the expiry
   in `preferences`. This is still needed — `check_local_cert_expiry` reads that field
   on the NEXT run. The two functions are complementary, not redundant.
5. Verify `kube_config.toml` fields all correspond to `Config`/`Server` struct fields
6. Check `Cargo.toml` for any lingering unused dependencies (e.g. `env_logger` if
   `flexi_logger` replaced it fully)
7. Verify no `String` holding a password value has a `Debug` derive that would print it

---

## Execution Order

```
Phase 0: Security
  0.1  Add keyring + rpassword to Cargo.toml
  0.2  Remove default_password from kube_config.toml
  0.3  Create src/credentials.rs (CredentialResult + public functions + KeyringBackend trait)
  0.4  Update fetch_remote_file in ssh.rs (new signature: + password param, sudo-S support)
  0.5  Add credential subcommand to main.rs CLI
       → cargo build (verify compiles)
    ↓
Phase 1+2: Cert Check + Merge (interleaved — 2.1 must precede 1.4)
  1.1  Add CertStatus to kube.rs
  1.2  Add check_local_cert_expiry to kube.rs
  1.3  Add SkipReason + ServerResult enums to main.rs
  2.1  Add merge_into_main_kubeconfig to kube.rs   ← must come before 1.4
  1.4  Rewrite process_server in main.rs (cert check + cred lookup + updated fetch call
       + call to merge_into_main_kubeconfig from 2.1)
       → cargo build
    ↓
Phase 3: Cron-Friendly Output
  3.1  Update process_servers result loop in main.rs
  3.2  Suppress progress bar when no fetches occurred
       → cargo build
    ↓
Phase 4: Tests
  4.1  Add cert expiry tests to tests.rs
  4.2  Add merge tests to tests.rs
  4.3  Add credential unit tests to tests.rs (using KeyringBackend mock)
  4.4  Add process_server early-return unit tests to tests.rs
       → cargo test (all must pass)
    ↓
Phase 5: /sc:improve
       → cargo test (re-verify after improvements)
    ↓
Phase 6: /sc:cleanup
       → cargo test (final verification)
    ↓
/sc:commit
```

---

## Files Affected

| File | Change type | Summary |
|---|---|---|
| `Cargo.toml` | Modified | Add `keyring`, `rpassword` |
| `kube_config.toml` | Modified | Remove `default_password`; add guidance comment |
| `src/credentials.rs` | **New** | `CredentialResult`, `get/set/delete/check_credentials`, `KeyringBackend` trait |
| `src/ssh.rs` | Modified | Add `password` param; password auth; `sudo -S` stdin write |
| `src/kube.rs` | Modified | Add `CertStatus`, `check_local_cert_expiry`, `merge_into_main_kubeconfig` |
| `src/main.rs` | Modified | Add `Commands/CredentialAction` enums; add `SkipReason/ServerResult` enums; rewrite `process_server`; update `process_servers` |
| `src/tests.rs` | Modified | Add tests for 4.1, 4.2, 4.3, 4.4 |

---

## Coherence Verification

The following cross-phase dependencies are confirmed consistent:

| Dependency | Produces | Consumed by |
|---|---|---|
| Phase 0.4 new `fetch_remote_file` signature (+ `password` param) | `ssh.rs` | Phase 1.4 `process_server` call site |
| Phase 0.3 `CredentialResult` enum | `credentials.rs` | Phase 1.4 credential lookup block |
| Phase 1.3 `SkipReason` + `ServerResult` | `main.rs` | Phase 3.1 results loop |
| Phase 1.2 `check_local_cert_expiry` writes `certificate-expires-at` | No — it **reads** it | Written by `add_cert_expiration` in `kube.rs` (existing, called inside `add_metadata` → `process_kubeconfig_file`). These are complementary, not conflicting. |
| Phase 2.1 `merge_into_main_kubeconfig` | `kube.rs` | Phase 1.4 step 9 — **2.1 must be implemented before 1.4** (execution order reflects this) |
| Phase 0.5 `credential list` | Needs config loaded | Uses same `config_path` resolution already in `main()` |

No circular dependencies. No phase produces something that contradicts another phase.

**Execution order note**: Phases 1 and 2 are interleaved in the execution order
(1.1 → 1.2 → 1.3 → **2.1** → **1.4**) because `process_server` (1.4) calls
`merge_into_main_kubeconfig` (2.1). The function must exist before the caller compiles.

---

## Checkpoints

- After Phase 0: `cargo build` — new SSH signature compiles, credential subcommand parses
- After Phase 1: `cargo build` — `process_server` returns `ServerResult`
- After Phase 2: `cargo build` — merge function callable from process_server
- After Phase 3: `cargo build` — results loop handles all variants
- After Phase 4: `cargo test` — all tests pass, including new ones
- After Phase 5: `cargo test` — improvements don't break tests
- After Phase 6: `cargo test` — final clean state

End-to-end manual test against a real server is viable after Phase 3 is complete.
