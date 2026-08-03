# ADR-0011 — Windows package and explicit project onboarding

**Status:** Accepted
**Scope:** Windows-first distribution after Phase 8.1

## Context

Phase 8.1 implemented safe MCP discovery and project hooks in the `lore` binary, but the repository did not yet provide a repeatable installation path. A package must be usable without Rust, must not duplicate integration logic, and must never discover repositories by recursively scanning the machine.

## Decision

1. The first distribution artifact is a versioned Windows ZIP containing `lore.exe`, PowerShell installers, and `.cmd` launchers for double-click/Command Prompt use.
2. The installer uses a user-local default directory, `%LOCALAPPDATA%\Lore\bin`, and adds only that directory to the user PATH. It does not require administrator privileges.
3. The installer accepts an explicit `-ProjectPath`. In interactive mode it may offer the current directory only when that directory contains `.git`; in non-interactive mode it never prompts or scans other directories.
4. The installer runs `lore setup --check` before any integration mutation. Applying MCP configuration and project hooks requires `-ApplySetup` or an interactive confirmation, and delegates all detection, ownership, backup, rollback, and handshake behavior to the binary.
5. The installer records the selected project in a local manifest so the uninstaller can remove Lore-owned hooks and integrations without guessing. Lore data under `LORE_HOME` is preserved by default.
6. Updates reuse the same idempotent installer and replace only the installed executable. No MSI-specific implementation is introduced until a Windows packaging tool and signing pipeline are authorized.
7. CI produces the ZIP and its SHA-256 as a Windows artifact. A public GitHub Release is the preferred first distribution channel; crates.io remains optional and is not the installation mechanism for post-install setup.
8. The current ZIP contains an unsigned executable (`Authenticode: NotSigned`). Signing is deferred until a certificate and an authorized pipeline exist; the published SHA-256 remains the integrity check in the meantime.

## Consequences

- Users can install and update Lore on Windows without Rust or a global machine change.
- Project scope remains explicit and auditable; installing Lore never scans drives or modifies unrelated repositories.
- The package is portable and reproducible with built-in PowerShell/Cargo tooling, but it is not an MSI yet.
- `cargo install --path . --locked` remains useful for contributors and local development, while release users can consume the ZIP.

## Validation

- PowerShell scripts parse successfully on Windows PowerShell/PowerShell 7.
- Package creation builds `lore.exe` with `cargo build --release --locked`, includes the expected files, and emits a SHA-256 hash.
- Installation into a temporary user-local directory is idempotent, updates the user PATH once, and supports `-SkipSetup`.
- Explicit project onboarding runs check before apply, does not scan outside the provided path, and records the project in the manifest.
- Uninstallation removes only the package binary/manifest and Lore-owned integrations, preserves `LORE_HOME`, and is idempotent.
