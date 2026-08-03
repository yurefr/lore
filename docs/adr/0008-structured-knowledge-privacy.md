# ADR-0008 — Structured knowledge without raw chat or chain-of-thought

**Status:** Accepted
**Date:** 2026-08-02

## Context

Storing everything recreates a memory manager and creates privacy risk. Lore's value is learning what deserves reuse, with explainable provenance and confidence.

## Decision

The Learning Engine produces `CandidateKnowledge`; only eligible candidates become `KnowledgeUnit`. Minimum fields are `goal`, `context`, `constraints`, `solution`, `artifacts`, `decision_summary`, `confidence`, `created_at`, and `provenance`. `decision_summary` is an objective justification, never detailed internal reasoning. The default mode is metadata-only; opt-in content passes through redaction and short retention.

## Consequences

- Recall returns structured experiences rather than transcripts.
- Confidence and completion remain independent policies.
- Deletion must reach the source, versions, and relations.

## Validation

Fixtures for incomplete candidates, below-threshold scores, redaction, idempotent promotion, deletion, and FTS without raw chat form the Phase 5 gate. Non-authoritative recall, ranking, and feedback remain Phase 6 and 7 gates.
