# ADR-0001 — Local-first runtime in a single process

**Status:** Accepted
**Date:** 2026-08-02

## Context

Lore must observe events in the background, maintain a durable inbox, and respond to connectors without requiring the user to run commands for every task. Microservices and an operating-system-installed daemon would increase distribution and support costs before real pressure exists.

## Decision

The product will be one Rust binary with modular modes. `serve` maintains the local instance and `lore mcp` starts or locates that process on demand, using the same `LORE_HOME` and an instance lock. `lore repair` revalidates migrations and hooks; `lore uninstall` requests cooperative shutdown through a local marker, removes managed hooks, and preserves data by default. Login-service installation and expansion to Linux/macOS remain outside the Windows-first gate.

## Consequences

- Operation and diagnostics remain simple (`init`, `status`, `doctor`, `repair`, `uninstall`, `serve`).
- Process failure can be recovered through the inbox without an external service.
- A future service installer must not change the core ports.
- The runtime does not promise automation when the connector lacks the corresponding capability.

## Validation

Concurrent locking, real `serve`, restart with a pending inbox, cooperative shutdown, and the on-demand startup E2E are gates for Phases 1, 2, 3, and 8. The current gate was executed only on Windows.
