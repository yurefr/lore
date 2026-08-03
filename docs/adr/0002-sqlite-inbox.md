# ADR-0002 — Local SQLite with a transactional inbox

**Status:** Accepted
**Date:** 2026-08-02

## Context

Storage must work without Postgres, an external queue, or a network connection, support migrations, and avoid duplication when hooks or connectors resend events.

## Decision

Bundled SQLite is the local storage engine. WAL, foreign keys, and a busy timeout are enabled. `schema_migrations` controls migrations; `inbox_events` is a transactional inbox keyed by `event_id`, with `status`, `attempts`, `processed_at`, and `last_error`. The inbox provides input durability; it is not Event Sourcing and is not the permanent source of truth for Knowledge Units.

## Consequences

- Capture can confirm quickly and process later.
- Phase 2 guarantees persistence/idempotency; the Phase 4 worker implements claim, bounded retry, `processing` recovery, and observable dead-letter handling.
- The Knowledge Store will have its own tables and explicit migrations.
- Corruption, locking, and migrations are diagnosable locally.

## Validation

Migration fixtures, duplicate inserts, restart between capture and processing, concurrent claims, and dead-letter handling will be exercised in Phases 2, 4, and 5.
