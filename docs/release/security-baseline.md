# Windows public release security baseline

**Status:** conditional release gate; no GitHub push, GitHub Release, or crates.io publication has been created by this task.

This checklist covers the first Windows ZIP publication. It is a release gate, not a security certification or a substitute for an independent review.

## Checks completed

- The repository contains no high-confidence credential patterns or sensitive named files. Redaction fixtures use explicit placeholder values only.
- `.agents/`, `.serena/`, `.rtk/`, and `target/` are excluded by the repository `.gitignore`, along with packaging outputs, common Windows IDE state, Rust formatter backups, and SQLite sidecars. A dry-run staging check does not include local state or generated caches/databases.
- The GitHub workflow grants `contents: read`, disables checkout credential persistence, and pins checkout, Rust toolchain, and artifact actions to immutable commit SHAs.
- `Cargo.lock` is present and `cargo tree --locked` resolves only registry dependencies; no path or Git dependency is used by the product package.
- `cargo audit` 0.22.2 loaded the RustSec database (1,186 advisories) and scanned 146 locked crate dependencies with no reported vulnerabilities.
- `cargo deny check advisories` 0.20.2 passed using its documented default configuration; no `deny.toml` is present.
- Gitleaks 8.30.1 scanned all 74 intended source/documentation candidates (including root files and public directories) with no leaks. Semgrep 1.172.0 ran 11 Rust/YAML rules with 0 findings, and actionlint 1.7.12 passed the workflow.
- `cargo fmt`, `cargo check`, Clippy with `-D warnings`, the complete test suite, and a locked release build pass on Windows. Check and Clippy emitted only an environment warning while finalizing stale incremental-session directories; no Rust lint or test warning was reported.
- The canonical target was used after the active `lore mcp`/`lore serve` processes were explicitly terminated. The complete suite passed with 71 tests across 9 suites, and the release build completed.
- `cargo package --list` contains source, documentation, and packaging files but no local agent state or runtime database.
- A high-confidence credential scan found only the intentionally synthetic redaction fixture at `src/domain/knowledge.rs:442`; Gitleaks found no leaks. No real credential pattern, sensitive filename, reparse file, or file larger than 10 MB was present in the candidate set.
- `git add --dry-run .` listed 74 intended product/documentation files and excluded `.agents/`, `.serena/`, `.rtk/`, and `target/`. Markdown verification covered 19 files with no broken relative links, replacement characters, or README status/dossier banner.
- The Windows package scripts parse successfully and define the six-entry ZIP contract. The final ZIP must be rebuilt from the local release tag; its SHA-256 and unsigned Authenticode state are reported with delivery.
- `publish = false` prevents accidental crates.io publication while the ZIP/GitHub Release remains the intended channel.

## Release-gate controls

- Review the exact staged file list immediately before the local commit and tag.
- Keep `cargo deny`, Gitleaks, Semgrep, and actionlint in the release validation set; the Windows run above is the current evidence.
- Build the release ZIP from the tagged commit and publish its SHA-256 alongside the asset; do not reuse a workspace-built archive without recomputing it from the tag.
- Add Authenticode signing and verify the signature when a certificate and authorized pipeline exist. Until then, any Windows binary must be presented as unsigned, not as trusted or certified.
- Keep Linux/macOS and additional provider formats outside the support claim until their own matrix passes.

## Release decision

The evidence supports a conditional public source publication after the final staged-file review: no real secret was found, the dependency advisory gates are clean, the Windows tests/build checks pass, and the CI actions are pinned. This is not a security certification. A Windows binary release remains conditional on building from the tagged commit, publishing the matching hash, and clearly disclosing that the artifact is unsigned; Linux/macOS and crates.io remain outside this gate.
