# Security Policy

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's private
security advisory flow for
[`parithosh/moneymoney`](https://github.com/parithosh/moneymoney/security/advisories/new)
and include the affected version or commit, impact, and a minimal reproduction.

Only the latest commit on `main` is supported while the fork has no published
release channel.

## Threat model

`mm` is a single-user, macOS-only CLI and stdio MCP server. It does not listen
on a network port and does not store bank credentials. MoneyMoney retains its
credentials and requires its database to be running and unlocked.

| Source | Trust | Treatment |
|---|---|---|
| MCP host process and startup environment | Trusted | Defines whether the process receives write capability. |
| MCP and CLI arguments | Untrusted | Validate financial fields and encode every AppleScript string. |
| Account and transaction data | Untrusted content | Return as structured data; never interpret it as AppleScript. |
| MoneyMoney plist structure | Trusted protocol, untrusted size | Parse only after bounded process execution and output capture. |
| Local paths | Untrusted | Canonicalize and restrict batch imports to MoneyMoney's container. |

## Write capability

Writes are disabled unless `MM_ENABLE_WRITES` is exactly `true` when `mm`
starts. In read-only MCP mode, write tools are hidden and rejected. This is a
capability boundary for an already-running agent process, not authentication
against arbitrary code already executing as the user.

Transfers still require review and TAN entry in MoneyMoney. `add_transaction`
and `set_transaction` mutate local financial records without a bank-side TAN,
so they use the same explicit write capability.

## Distribution

The plugin does not download executables or remove `com.apple.quarantine`.
Build the reviewed source locally with `cargo install --path . --locked`.
Prebuilt binaries must not be introduced without verified provenance and the
normal macOS Developer ID signing and notarization path.

## Platform boundaries

macOS Transparency, Consent, and Control governs Automation access to
MoneyMoney. `mm` invokes `/usr/bin/osascript` and uses stdio for MCP. Adding a
network transport requires a new threat model, authentication, authorization,
TLS, origin controls, and request limits; it is not an incremental change.
