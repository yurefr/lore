# Retrieval evaluation — Phase 6

## Frozen baseline

- Versioned dataset: `tests/fixtures/retrieval_dataset.json` (`phase6-v1`).
- Primary metric: `hit_at_1` for queries with a known relevant Knowledge Unit.
- Lexical baseline: SQLite FTS5 with the same filters and budget.
- Initial provider: `lore-hash-v1`, 128-dimensional, deterministic, local, and network-free.
- Fusion: Reciprocal Rank Fusion (`K=60`) with lexical, semantic, scope, confidence, recency, artifact, and relation decomposition.
- Reuse: append-only feedback aggregates `used`, `ignored`, and `corrected`; the signal is separate from the base score and explained in `scores.feedback`/`why_selected`.

The minimum set contains a lexical query, a paraphrase without the same terms, a scope case, and an irrelevant case. The integration test runs relevant queries against the hybrid service and `--lexical-only`; hybrid retrieval must be equal to or better than the baseline, and the paraphrase must be retrievable.

## Acceptance criteria

1. `semantic_available` is true only when at least one candidate receives a semantic signal.
2. `lexical_fallback` is true when the provider is absent, fails, or produces no usable vector.
3. Every result contains reproducible `scores` and `why_selected`.
4. Project/global, artifact, and confidence filters are applied before ranking.
5. Changing `model_id` or dimension preserves Knowledge Units and vectors from the previous version.
6. Migration v5 is applied automatically to a v4 database.
7. After feedback, ranking changes deterministically without changing the Knowledge Unit or its provenance; without feedback, `scores.feedback` is zero.

## Known limits

The embedded provider is a hashing baseline over normalized terms and trigrams; it is not intended to replace a trained neural model. Choosing a neural provider, benchmarking license/size, and establishing a sustained memory/latency budget remain hardening work and do not make local search network-dependent.
