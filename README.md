# Lore

Lore is a local-first knowledge engine for AI agents. It observes work evidence, keeps only structured and explainable knowledge, and returns relevant context through a provider-neutral protocol.

## Principles

- The core does not depend on Codex, Claude, Cursor, MCP, SQLite, or a particular embedding model.
- Capture defaults to `metadata_only`; raw chats are not stored as Knowledge Units.
- Hooks, connectors, and watchers translate evidence; business rules remain in the core.
- Knowledge is structured, versioned, explainable, privacy-aware, and non-authoritative.
- The basic runtime is local and does not require a remote service.

## Current capabilities

Validated on Windows:

- Rust 2024 package with MSRV 1.85.
- Local configuration through `LORE_HOME` (default: `%USERPROFILE%\.lore`).
- SQLite WAL storage, migrations, durable inbox, locking, and structured diagnostic logs.
- Git hooks for `post-commit`, `post-merge`, and `post-checkout`, with reversible composition.
- Filesystem watcher with 500 ms aggregation and ignored-directory filters.
- Idempotent event capture and deduplication by `event_id`.
- Learning Sessions, completion/confidence policies, retry/dead-letter processing, provenance, and automatic promotion.
- Knowledge Store with versioning, redaction, retention, FTS5, and transactional deletion.
- Hybrid retrieval with local deterministic `lore-hash-v1` embeddings, explainable rank fusion, filters, reindexing, and lexical fallback.
- Automatic context recall for `task.start`, bounded by a deterministic context budget and marked non-authoritative.
- MCP stdio server with capabilities, `lore_task_start`, `lore_task_end`, `lore_recall`, and `lore_feedback`.
- On-demand local runtime startup from `lore mcp`; an always-running `serve` process is not required for normal connector use.
- Safe MCP setup for Codex, Claude, and Gemini, including read-only checks, ownership markers, backups, rollback, handshake validation, and selective removal.

## Supported platform

Windows is the only platform in the current release gate. Linux and macOS are not declared supported until their own compatibility matrix covers the runtime, watcher, hooks, paths, and provider configuration.

## Installation on Windows

### Release package (recommended for daily use)

The repository produces a versioned ZIP containing `lore.exe`, an installer, and an uninstaller. The package uses a user-local install directory (`%LOCALAPPDATA%\Lore\bin`) and does not require administrator privileges.

Build the package from a clone:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\packaging\windows\package.ps1
```

The script builds `target\release\lore.exe`, creates `dist\lore-<version>-windows-x86_64.zip`, and prints its SHA-256 hash.

Extract the ZIP and install it:

```powershell
Expand-Archive .\lore-0.1.0-windows-x86_64.zip -DestinationPath .\lore-package
Set-Location .\lore-package
pwsh -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
```

The same package includes `install.cmd` and `uninstall.cmd` wrappers for Windows Command Prompt or double-click use.

The interactive installer offers the current directory only when it is a Git repository. To target a known project explicitly:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 `
  -ProjectPath C:\work\my-project `
  -ApplySetup
```

`-ApplySetup` is explicit consent to change the selected provider configuration and install Lore hooks in that project. Without it, the installer runs `lore setup --check` and leaves configuration unchanged.

The installer never scans drives, recursively searches for Git repositories, or changes unknown provider configuration. It does not edit `AGENTS.md` by default. MCP configuration is checked only at the known user-level paths for the supported providers; hooks are scoped to the selected project.

### Install from source with Cargo

This is the simplest option for contributors while no public release is published:

```powershell
git clone <repository-url>
Set-Location Lore
cargo install --path . --locked

lore setup --check --path C:\work\my-project
lore setup --apply --yes --path C:\work\my-project
```

`cargo install` installs the binary but does not run onboarding automatically. The explicit `setup` commands are still required.

## First-run and daily commands

```powershell
# Inspect the current provider and hook state without writing files
lore setup --check --path C:\work\my-project

# Configure supported MCP providers and install project hooks after consent
lore setup --apply --yes --path C:\work\my-project

# Optionally manage the Lore block in the known Codex global AGENTS.md
lore setup --apply --yes --agent-instructions

# Inspect foundation and integration diagnostics
lore doctor --integrations

# Run the local runtime once or continuously
lore serve --once
lore serve

# Repair migrations and managed hooks without deleting knowledge
lore repair --path C:\work\my-project

# Search and recall structured knowledge
lore search "fix authentication issue" --project-id <PROJECT_ID>
lore recall "repair login problem" --project-id <PROJECT_ID> --budget 5
```

`search` and `recall` support project/global scope, artifact filters, minimum confidence, lexical-only fallback, and reindexing. The semantic provider is local and deterministic; no model download is required.

`--agent-instructions` is opt-in and targets `CODEX_HOME\AGENTS.md` (or `%USERPROFILE%\.codex\AGENTS.md`). Lore inserts an ownership-marked, metadata-only workflow block, preserves unmarked instructions, creates a `.lore-original` backup before changing an existing file, and is idempotent. Use `lore setup --check --agent-instructions` for a read-only preview and `lore setup --remove --yes --agent-instructions` to remove only Lore-owned content.

## Update and uninstall

Run the same package installer with a newer ZIP to update the binary. Installation is idempotent and replaces only the installed `lore.exe`.

To repair an existing project:

```powershell
lore repair --path C:\work\my-project
```

To remove Lore from the package installation:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\uninstall.ps1 `
  -ProjectPath C:\work\my-project
```

The uninstaller removes only Lore-owned MCP entries, managed hooks, the package binary, and its user PATH entry. Lore data under `LORE_HOME` is preserved by default. Data deletion remains an explicit CLI operation (`lore uninstall --purge-data`).

## Distribution status

There is no published crates.io release or signed MSI yet. The current distribution choices are:

- Use the Windows ZIP for a repeatable end-user installation.
- Use `cargo install --path . --locked` from a clone for development and local testing.
- Publish the ZIP as a GitHub Release before considering crates.io. A GitHub Release can carry the archive and SHA-256 without making `cargo install` responsible for provider setup.

The current Windows ZIP contains an unsigned executable (`Authenticode: NotSigned`). Verify the SHA-256 published with each release before running it.

The eventual MSI, if adopted, should call the existing `install.ps1`/`lore setup` flow rather than reimplement detection. Authenticode signing will be added only when a certificate and authorized pipeline exist.

## Architecture

```text
Agent / connector
        │ Lore Protocol v1
        ▼
Capture + SQLite inbox ◄── Git hooks / filesystem watcher / CLI
        │
        ▼
Learning Session → CandidateKnowledge → KnowledgeUnit
        │
        ▼
Hybrid retrieval → Context Builder → agent
```

The package is organized into `domain`, `application`, `infrastructure`, and `interfaces`, using ports and adapters without a dependency-injection container or abstractions without a second use.

## Documentation

- [Lore Protocol v1](docs/rfc/lore-protocol-v1.md)
- [Privacy policy](docs/policies/privacy.md)
- [Completion and confidence policies](docs/policies/completion-confidence.md)
- [Requirements traceability](docs/requirements-traceability.md)
- [Codex/MCP integration](docs/integrations/codex-mcp.md)
- [MCP auto-setup ADR](docs/adr/0010-mcp-provider-auto-setup.md)
- [Windows package/onboarding ADR](docs/adr/0011-windows-package-onboarding.md)
- [Windows release security baseline](docs/release/security-baseline.md)
- [All ADRs](docs/adr/)

## License

MIT. See [LICENSE](LICENSE).
