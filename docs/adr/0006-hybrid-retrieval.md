# ADR-0006 — Explainable hybrid retrieval

**Status:** Accepted
**Date:** 2026-08-02

## Context

Lexical search alone misses paraphrases; semantic search alone is difficult to explain and does not cover project, artifact, or confidence filters.

## Decision

Retrieval combines SQLite FTS5, optional semantic signals, structured filters, and relations. The first implementation uses the deterministic local provider `lore-hash-v1`, with a normalized 128-dimensional vector, no download, and no mandatory external service. The canonical vector representation contains only the already-redacted `goal`, `context`, `constraints`, `solution`, and `artifacts` fields of a Knowledge Unit.

Rank fusion is deterministic Reciprocal Rank Fusion (RRF), with `K=60` and explicit weights for lexical, semantic, scope, confidence, recency, artifact, and relation signals. Results return `why_selected` and the signal breakdown. The current project is prioritized, with global scope explicit. When the provider is absent or unavailable, or a vector is missing, FTS5 remains functional and the result marks `lexical_fallback=true`.

Embeddings are stored by `(knowledge_id, version, model_id)`. Changing the model ID or dimension creates a new vector series and updates only the active state; old vectors and Knowledge Units are never deleted during reindexing.

## Consequences

- Quality is measured in `tests/fixtures/retrieval_dataset.json`, using `hit_at_1` and comparison with the lexical baseline rather than manual impressions.
- A remote model is never a recall requirement.
- A model change requires versioning, safe reindexing, and comparison with the lexical baseline.

## Validation

The dataset covers terms, paraphrases, project scope, and irrelevant results. The Windows-first gate measures `hit_at_1`, verifies that hybrid search does not fall below the lexical baseline, tests fallback, filters, explanations, migration v5, and vector preservation during provider changes. Latency and memory remain hardening budgets because the initial provider is embedded and fixed-dimension.
