# ADR-0007 — Optional local embeddings with explicit downloads

**Status:** Accepted
**Date:** 2026-08-02

## Context

Embeddings improve paraphrase retrieval, but an embedded model increases the binary size and a silent download violates predictability and privacy.

## Decision

`EmbeddingProvider` is a port. Phase 6 delivers `lore-hash-v1`, an embedded deterministic local provider with 128 dimensions, no download, no external model license, and no network. It is a replaceable operational baseline and does not prevent future adoption of a local neural model.

Future model-backed providers remain versioned and licensed and may download only after explicit consent, with a checksum and an isolated directory. First use will never download silently. If the provider is absent or fails, Retrieval degrades to FTS5. Reindexing writes the new version without removing old vectors.

## Consequences

- Basic operation does not depend on a network.
- The embedded baseline has predictable cost; provider updates schedule reindexing without deleting existing units.
- License, size, latency, and quality of any future neural model require benchmarking before distribution.

## Validation

The Phase 6 gate tests an absent provider, lexical fallback, versioned dimensions/model IDs, safe migration, and retention of old vectors. Checksum, license, memory, and latency for a downloadable provider are gates for future model-backed adoption, not hidden requirements of the embedded baseline.
