# Initial validation — Codex through MCP

**Status:** homologation fixture for Phases 3, 6, and 7
**Client:** Codex
**Validated platform:** Windows

Codex is the first client used to validate the connector, but the protocol does not depend on Codex APIs or internal decisions. The process is local and speaks JSON-RPC/MCP over stdio. The Windows-first implementation accepts MCP versions `2024-11-05`, `2025-03-26`, and `2025-06-18`, including the version currently sent by the validated Codex host.

## Conceptual configuration

Adapt the format to the host's MCP configuration file:

```json
{
  "mcpServers": {
    "lore": {
      "command": "D:\\Projects\\Dev\\GITHUB\\Lore\\target\\debug\\lore.exe",
      "args": ["mcp"],
      "env": {
        "LORE_HOME": "C:\\Users\\<user>\\.lore"
      }
    }
  }
}
```

With a Windows-first installation, manual registration can be replaced by the safe flow:

```powershell
lore setup --check --provider codex --path .
lore setup --apply --yes --provider codex --path .
lore doctor --integrations
```

To optionally add Lore lifecycle guidance to the known global Codex instruction file, run `lore setup --check --agent-instructions` and then `lore setup --apply --yes --agent-instructions`. This targets `CODEX_HOME/AGENTS.md` (or `%USERPROFILE%\\.codex\\AGENTS.md`), uses an ownership marker and backup, and preserves unmarked instructions.

The adapter reads `%CODEX_HOME%\config.toml` (or `%USERPROFILE%\.codex\config.toml`), preserves other servers, and writes the `LORE_MANAGED_BY = "lore"` marker only to the entry created by Lore. `--check` writes nothing; `--apply` requires `--yes`; `--remove --yes` removes only an entry with that marker. The original file is kept as `config.toml.lore-original` for restoration.

For distribution, replace the development path with the installed binary. When started by the host, `lore mcp` locates the existing runtime or starts `lore serve` in the background with the same `LORE_HOME`; running `serve` manually for every session is not required. The connector must not download a model, send data over the network, or store credentials.

## Expected handshake

The client sends `initialize` with a supported MCP `protocolVersion` and `clientInfo`. Lore responds with:

- `loreProtocolVersion = 1`;
- capabilities `event_ingest`, `task_lifecycle`, `recall`, and `feedback` when the corresponding services are composed;
- `automationLevel = capture_and_lifecycle` when the client declares both capabilities;
- `tools/list` containing `lore_event_ingest`, `lore_task_start`, `lore_task_end`, `lore_recall`, and `lore_feedback`.

`lore_recall` returns hybrid results, scores, and `why_selected`; without a provider or usable vector, the result marks `lexical_fallback=true`. `lore_task_start` accepts `metadata.query`, `metadata.goal`, or `metadata.task` and returns an optional `ContextPackage`, with `authority=non_authoritative_context`, a maximum budget of 20, and deduplication before the limit. `lore_feedback` records `used`, `ignored`, or `corrected` separately from the Knowledge Unit's provenance and influences future ranking. The client must present the knowledge as non-authoritative context.

## Homologation scenario

1. Initialize a Git project and run `lore init`.
2. Start the MCP process through the host; the connector starts the local runtime on demand.
3. Confirm `initialize`, `notifications/initialized`, and `tools/list`.
4. Call `lore_task_start` and retain the `session_id`.
5. Send `lore_event_ingest` or allow hooks/the watcher to capture evidence.
6. Call `lore_task_end` with `success`, `failed`, or `cancelled`.
7. Repeat `lore_task_start` with `metadata.goal` and inspect the `ContextPackage`; without a query, `context` must be omitted.
8. Confirm that the inbox contains the events and that `lore_recall` returns a result or a structured lexical fallback.
9. Send `lore_feedback` with `used`, `ignored`, or `corrected` and verify the append-only record.
10. Repeat the handshake with an unknown MCP version and confirm the `unsupported_protocol_version` error.

## Current limits

- Phases 3 and 6 validated transport, lifecycle, and hybrid retrieval; Phase 7 added Context Builder, auto-recall, and feedback.
- The connector accepts `metadata_only`; raw content is rejected with `privacy_violation`.
- Connector failure must not block Git, editing, or the agent.
- `lore repair` revalidates migrations and hooks without deleting knowledge; `lore uninstall` stops the runtime and preserves data by default.
- On-demand startup and the lifecycle commands documented here were validated only on Windows-first.
