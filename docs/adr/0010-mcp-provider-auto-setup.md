# ADR-0010 — Safe MCP provider discovery and setup

**Status:** Accepted
**Phase:** 8.1 — Windows-first

## Context

Lore must be usable after installation without requiring the user to learn each client's configuration format. Codex, Claude, and Gemini store MCP registrations in different files and formats. Existing configuration, custom Git hooks, and unknown providers must never be blindly overwritten.

## Decision

1. The infrastructure layer contains configuration adapters that convert each format to a shared `McpRegistration`. The core and protocol do not know about Codex, Claude, or Gemini.
2. `lore setup --check` and `lore doctor --integrations` are read-only. They detect known user configuration files, inspect format, permissions, conflicts, ownership, and run an isolated MCP handshake.
3. `lore setup --apply --yes` changes only missing entries or entries marked as managed by Lore. Writing creates a `.lore-original` backup (or a versioned backup when the file has evolved), writes through a temporary file, and is idempotent.
4. `lore setup --remove --yes` removes only an entry containing `LORE_MANAGED_BY = lore`; third-party entries and an unmarked Lore-looking configuration are preserved.
5. The installer and default setup do not change `AGENTS.md`, instruction files, or unknown providers. The explicit `lore setup --apply --yes --agent-instructions` option may manage the known Codex global `CODEX_HOME/AGENTS.md` block with ownership markers, idempotence, backups, and preservation of unmarked instructions. Other instruction files remain untouched; for a client without an adapter, diagnostics provide a manual snippet.
6. MCP configuration is user-scoped in this phase. Hooks are project-scoped and are installed only in the path selected by `--path` (or the current directory). No arbitrary directory is scanned.
7. Hook resolution uses `git rev-parse --git-path hooks`, covering `core.hooksPath` and worktrees, with a controlled `.git/hooks` fallback for fixtures or environments without the Git executable.
8. Setup handshakes use `LORE_DISABLE_ON_DEMAND=1` only in the probe process, avoiding an auxiliary runtime or orphaned process during a read-only diagnostic.

## Consequences

- Installation can offer automatic configuration after an initial confirmation; automated installations use explicit `--yes`.
- Agent instructions remain opt-in and user-scoped; the default MCP/provider setup cannot silently change agent policy files.
- TOML/JSON formats are semantically preserved but serialized again when a change is applied; the backup allows restoration.
- An installed provider without a configuration file is configured only when its executable is detectable or the file already exists; undetected providers receive diagnostics only.
- Initial support remains Windows-first. New configuration paths and platform matrices require their own tests before being declared stable.

## Validation

`tests/phase81_windows.rs` covers missing configuration, non-mutating check, handshake, idempotent application, backup/removal, conflicts, invalid configuration, a project without Git, `core.hooksPath`, `doctor --integrations`, and the opt-in global agent-instructions flow. Unit tests cover managed-block idempotence, backup/removal, read-only detection, and preservation of unowned Lore instructions.
