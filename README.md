# moneymoney

Agent-native CLI (`mm`) and MCP server for
[MoneyMoney](https://moneymoney.app/).

Query accounts, transactions, categories, portfolios, and bank
statements through a single Rust binary that works as:

- A standalone command-line tool (`mm accounts list`, `mm transactions --account …`, …)
- An MCP server over stdio (`mm mcp`) for Claude Desktop and other MCP hosts
- Claude Code and Codex plugins that vendor both

## Install

Build the reviewed source locally:

```bash
git clone https://github.com/parithosh/moneymoney.git
cd moneymoney
cargo install --path . --locked
```

The Claude Code and Codex plugins require that independently installed `mm`
binary on `PATH`; their shim never downloads or executes release artifacts.

| Plugin | Command |
|---|---|
| Claude Code | `/plugin marketplace add parithosh/moneymoney`<br>`/plugin install moneymoney@moneymoney` |
| Codex | `codex plugin marketplace add parithosh/moneymoney`<br>`codex plugin add moneymoney@moneymoney` |

There is currently no prebuilt release, Homebrew tap, or fork-owned crates.io
package. Do not use bare `cargo install moneymoney` to install this fork.

## 30-Second Tour

```bash
mm status                                 # is MoneyMoney running + unlocked?
mm accounts list                          # leaf accounts
mm accounts list --tree                   # full sidebar hierarchy
mm accounts get "ING/Girokonto"           # one account, full details
mm transactions --account "ING/Girokonto" --from 2026-01-01
mm portfolio --account "Trade Republic/Wertpapierdepot"
mm statements list --since 2026-01-01
```

Output defaults to **table** in a TTY and **JSON** when piped. Override
with `-o json|ndjson|table`. Filter fields with `-F bank,name,iban,balance`.
For SEPA accounts, `iban` is the normalized (no-whitespace, mod-97-verified)
IBAN; for PayPal or legacy accounts the field is absent.

## Writing

Writes are disabled unless `MM_ENABLE_WRITES` is exactly `true` when `mm`
starts:

```bash
MM_ENABLE_WRITES=true mm transfer create \
    --from "ING/Girokonto" --to DE89... --amount 12.34 \
    --purpose "Rent" --into-outbox
MM_ENABLE_WRITES=true mm transfer direct-debit \
    --from "ING/Girokonto" --to DE89... --amount 500 --mandate MANDATE-42
MM_ENABLE_WRITES=true mm transfer batch path/to/sepa.xml --direct-debit
MM_ENABLE_WRITES=true mm transaction add \
    --account "Cash" --date 2026-04-20 --name "Coffee" \
    --amount -3.50 --category "Food\\Coffee"
MM_ENABLE_WRITES=true mm transaction set 12345 --category "Food\\Groceries"
```

The batch XML path is canonicalized and must resolve inside MoneyMoney's app
container. Recipient and debtor IBANs are mod-97 validated, and transfer
amounts must be positive.

Transfers never move money silently: MoneyMoney opens a pre-filled payment
window or parks the payment in the outbox, and you confirm and enter a TAN in
the GUI. Write subcommands are also omitted from the skill's `allowed-tools`,
so agent-driven CLI calls require host permission.

## Account References

`<REF>` is any of (priority order, first match wins):

1. **UUID** — always unique
2. **IBAN** — mod-97 validated via the `iban_validate` crate
3. **Account number** — PayPal emails, legacy digits
4. **Alias** — see config
5. **`Bank/Name`** path — e.g., `"ING/Girokonto"`
6. **Bare name** — only when unambiguous across banks

Ambiguous bare names (e.g., two "Girokonto" accounts) return an
`ambiguous_account` error listing both candidates as `Bank/Name` paths.

## Config (Optional)

`~/.config/mm/config.toml`:

```toml
[aliases]
checking = "ING/Girokonto"
depot    = "Trade Republic/Wertpapierdepot"
pp       = "PayPal"
```

`mm` itself holds **no credentials**. MoneyMoney owns every bank
secret in its encrypted store; `mm` just shells out to its AppleScript
surface.

## MCP

```bash
mm mcp                                    # read-only stdio server by default
```

Register with Claude Desktop
(`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "moneymoney": {
      "command": "/path/to/mm",
      "args": ["mcp"]
    }
  }
}
```

Read tools: `status`, `list_accounts`, `get_account`, `list_transactions`,
`list_categories`, `get_portfolio`, `list_statements`, `get_statement`.

Write tools are hidden and rejected by default. To enable them for the entire
server process, set the capability in the host configuration:

```json
{
  "mcpServers": {
    "moneymoney": {
      "command": "/path/to/mm",
      "args": ["mcp"],
      "env": {
        "MM_ENABLE_WRITES": "true"
      }
    }
  }
}
```

Enabled write tools: `create_transfer`, `create_direct_debit`,
`create_batch_transfer`, `add_transaction`, and `set_transaction`.
`set_transaction` silently overwrites checkmark/category/comment metadata;
the other financial-write caveats are described above.

The MCP transport is local stdio only. The crate disables rmcp's default
features and enables no HTTP server transport.

## Security

Every dynamic AppleScript string uses one encoder. `/usr/bin/osascript`
execution has a fixed timeout and bounded stdout/stderr capture. The plugin
does not download executables or remove macOS quarantine metadata.

See [SECURITY.md](SECURITY.md) for the threat model and private reporting path.

## Platform Support

**macOS only at runtime.** AppleScript is the sanctioned MoneyMoney
interface and has no equivalent elsewhere. Builds are cross-platform
(Linux compiles cleanly) but runtime commands return `not_supported`
off macOS.

## Logging

All diagnostic output goes to **stderr**. stdout is reserved for
structured results (CLI) or JSON-RPC frames (MCP). Control verbosity
through `MM_LOG` (any `tracing_subscriber::EnvFilter` syntax):

```bash
MM_LOG=mm=debug mm accounts list
```

## Build & Test

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

**MSRV:** 1.94 (edition 2024).

## License

MIT
