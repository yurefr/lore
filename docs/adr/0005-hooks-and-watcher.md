# ADR-0005 — Automatic capture through non-blocking hooks and a watcher

**Status:** Accepted
**Date:** 2026-08-02

## Context

The agent does not always emit every event. Git and the filesystem provide complementary evidence, but existing hooks must not be destroyed and physical changes must not create an event storm.

## Decision

The MVP installs wrappers for `post-commit`, `post-merge`, and `post-checkout`. An existing hook is saved as `.lore-original`, executed before Lore, and restored by `hooks remove`. `pre-commit` is excluded without a specific justification. The `notify` watcher aggregates by project in an initial 500 ms window and ignores `.git`, `.lore`, `target`, and `node_modules`. Capture is `metadata_only`, and any Lore failure is non-blocking for Git or editing.

## Consequences

- Capture starts after initial setup without a recurring command.
- Hooks do not capture conversations; a connector remains necessary for agent events.
- Composition is reversible and requires tests for an existing hook, a real commit, and removal.
- The aggregation window and sensitive-name policy can be recalibrated without changing the envelope.

## Validation

The Phase 2 Windows-first smoke test covers installation, backup, commit, aggregation, ignored directories, Lore unavailability, and restoration.
