# ADR-0001: Use Bitwarden Password Manager Over Secrets Manager

## Status

Accepted

## Context

We need a centralized vault backend for company-managed SSH credentials and server configurations. Bitwarden offers two products:

1. **Password Manager** — the traditional vault with Login items, custom fields, and organization/collection-based access control. Accessed via the `bw` CLI. Works with both Bitwarden Cloud and self-hosted Vaultwarden.

2. **Secrets Manager** — purpose-built for machine credentials. Accessed via the `bws` CLI or the `bitwarden` Rust crate. Requires Bitwarden Cloud or Bitwarden Enterprise — **not available on self-hosted Vaultwarden**.

The primary deployment target is a company running Vaultwarden (self-hosted). Users already have Bitwarden vaults they log into daily.

## Decision

We will use the **Bitwarden Password Manager** (`bw` CLI) as the vault backend.

Each k3s server is stored as a Login item in a Bitwarden collection:
- `login.username` → SSH user
- `login.password` → SSH password
- Custom fields → server config (`address`, `target_cluster_ip`, `file_path`, `file_name`, `context_name`, `identity_file`)

This was chosen over Secrets Manager because:
- It works with Vaultwarden — the primary deployment target
- Users already have it — no additional product or license
- The Login item type maps naturally to SSH credentials
- Custom fields provide structured config storage without JSON conventions
- Organizations + Collections provide the access control model the company needs

Secrets Manager was rejected because it requires Bitwarden Cloud or Enterprise (not available on Vaultwarden), adds a paid license requirement, and its flat key-value model is a worse fit for structured server configs.

A hybrid approach (Password Manager for config, Secrets Manager for passwords) was rejected because it doubles the integration surface for no meaningful security gain — the Password Manager already stores passwords securely.

## Consequences

**Positive:**
- Works identically with Bitwarden Cloud and self-hosted Vaultwarden
- No new product dependencies or license costs
- Leverages existing user accounts and organization permissions
- Access control is delegated to Bitwarden — the tool doesn't implement authorization

**Negative:**
- Session management is heavier than Secrets Manager's access tokens — requires a two-phase login+unlock flow
- The `bw` CLI is a Node.js application (~100MB) that must be installed separately
- Custom fields are less structured than a purpose-built API — field naming conventions must be documented

**Neutral:**
- The `bw` CLI becomes a runtime dependency when the `[bitwarden]` feature is enabled
- Vault items must follow a naming convention (`k3s:` prefix) for discovery
