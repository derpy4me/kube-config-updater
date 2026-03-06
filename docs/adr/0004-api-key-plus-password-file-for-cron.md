# ADR-0004: API Key + Password File for Cron/Headless Authentication

## Status

Accepted

## Context

The Bitwarden CLI requires a two-phase authentication flow: login (authenticates to the server) then unlock (decrypts the local vault cache). Both require interactive input by default — a master password or 2FA code. Cron jobs run in a fresh shell with no user present.

Four approaches were evaluated for unattended authentication:

1. **Inline env var** — `BW_PASSWORD` in the cron environment
2. **API key login + password file** — `BW_CLIENTID`/`BW_CLIENTSECRET` env vars + `--passwordfile` for unlock
3. **`bw serve` daemon** — persistent local REST API on localhost
4. **Local cache** — periodic sync to an encrypted cache file

## Decision

We will use **API key login + password file unlock** as the primary headless authentication method.

The flow:
1. `BW_CLIENTID` and `BW_CLIENTSECRET` environment variables provide API key credentials (set by cron wrapper or systemd)
2. `password_file` in `[bitwarden]` config points to a `chmod 600` file containing the master password
3. The tool runs `bw login --apikey --nointeraction` then `bw unlock --raw --passwordfile <path>`
4. The session key is captured and held in memory for the duration of the run

The detection cascade in `BwCli::ensure_session`:
1. `BW_SESSION` env var → use directly (already unlocked)
2. `BW_CLIENTID` + `BW_CLIENTSECRET` + `password_file` → headless login+unlock
3. Neither → return error (TUI handles interactive prompt separately)

This was chosen because:
- API key bypasses 2FA prompts — essential for unattended use
- `--passwordfile` keeps the master password out of environment variables and process arguments
- Personal API keys work with both Bitwarden Cloud and Vaultwarden (confirmed)
- API keys are rotatable without changing the master password
- The security model (secrets in `chmod 600` files) matches the existing file-based credential fallback

**Inline env var (Option A)** was rejected because the master password is visible in `/proc/<pid>/environ` — poor security for production cron jobs.

**`bw serve` daemon (Option C)** was rejected for v1 because it adds operational complexity (systemd service management, always-unlocked vault in memory, no auth on the HTTP API). It remains a viable future enhancement for high-frequency polling use cases.

**Local cache (Option D)** was rejected because it introduces a two-stage system (sync job + main tool), cache encryption key management, and stale data between syncs.

## Consequences

**Positive:**
- Fully headless — no interactive prompts needed
- Master password never in env vars or CLI args
- API key is rotatable independently of the master password
- Works with both Bitwarden Cloud and Vaultwarden
- Simple: same `bw` CLI, different auth flags

**Negative:**
- Master password stored on disk in a file (mitigated by `chmod 600` + ownership)
- Two credential files to manage (`bw-credentials` for API key, `bw-password` for master password)
- `bw` CLI must be installed on the cron host
- The tool warns (like SSH does for private keys) if `password_file` has overly permissive permissions, but cannot prevent a misconfigured system

**Neutral:**
- A cron wrapper script is documented in the README but is not part of the binary — users manage their own cron setup
- `password_file` is optional in config — when absent, only `BW_SESSION` env var or interactive TUI unlock works
