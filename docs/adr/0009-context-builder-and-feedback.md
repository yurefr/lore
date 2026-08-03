# ADR-0009 — Non-authoritative Context Builder and append-only feedback

**Status:** Accepted
**Date:** 2026-08-02

## Context

Retrieval produces explainable candidates, but the agent needs a bounded package without duplicates and must be able to report whether knowledge was useful. The package must not compete with the user's current message or retroactively change a unit's provenance.

## Decision

`ContextBuilder` is an application service that calls `RetrievalService`, applies a deterministic budget (default 5, maximum 20), prioritizes current-project entries, deduplicates `knowledge_id`/version and `content_hash`, and emits `ContextPackage.authority = "non_authoritative_context"`. Each entry contains scope, structured summary, confidence, provenance, and `why_selected`.

`task.start` extracts a query only from `metadata.query`, `metadata.goal`, or `metadata.task`; without a query, the lifecycle remains valid without context. The connector translates the package but does not turn it into a system instruction.

`feedback` records a separate `KnowledgeUsage` entry for `used`, `ignored`, or `corrected`. The unit and its provenance are not rewritten. Retrieval aggregates these records into a deterministic signal, positive for reuse and negative for ignored/corrected results, while preserving the previous ranking when no feedback exists.

## Consequences

- Automatic context is useful and bounded without introducing an LLM or external service.
- Transient Retrieval unavailability does not block `task.start`; the lifecycle event remains durable and the package is omitted.
- Current-project ordering is stable even when a global result has a higher semantic score.
- Feedback is auditable and can be removed with the unit through a foreign key.
- The reuse score is an operational baseline; recalibration requires a dataset and future review.

## Validation

Budget, authority, scope, deduplication, auto-recall, missing-query, append-only persistence, provenance isolation, and ranking-signal tests form the Phase 7 gate. The MCP smoke test verifies `task.start` with context and `lore_feedback` without error.
