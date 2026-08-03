# FR/NFR traceability matrix

This matrix links each dossier requirement to an owning phase and planned validation evidence. A requirement may be marked complete only when the corresponding evidence exists in the phase gate.

## Functional requirements

| ID | Owning phase | Planned evidence/test |
|---|---|---|
| FR-001 | Phase 1 | idempotent `init` and non-overwrite test |
| FR-002 | Phase 3/8 | `tests/phase8_windows.rs`: `lore mcp` starts `lore serve` on demand; Windows-first gate |
| FR-003 | Phase 2/3 | envelope contract test, idempotency by `event_id` |
| FR-004 | Phase 4 | Learning Session correlation tests |
| FR-005 | Phase 2 | real commit triggers hooks and persists an event |
| FR-006 | Phase 2 | watcher burst/coalescing test |
| FR-007 | Phase 4 | CompletionPolicy transition table |
| FR-008 | Phase 4 | structured candidate and auditable `decision_summary` |
| FR-009 | Phase 4 | signal-based score, positive/negative cases, and determinism |
| FR-010 | Phase 5/6 | below-threshold candidate is not promoted; Retrieval queries only promoted `knowledge_units` |
| FR-011 | Phase 5 | `knowledge_units` preserves provenance, version, confidence, fingerprint, and declared relations |
| FR-012 | Phase 6 | FTS5 + `lore-hash-v1` + structured filters; `tests/retrieval.rs` |
| FR-013 | Phase 6 | dataset and project/global scope integration |
| FR-014 | Phase 6 | deterministic RRF, `scores`, and `why_selected` |
| FR-015 | Phase 7 | `ContextBuilder`, default/maximum budget, project-before-global priority, and version/hash deduplication; `tests/context.rs` |
| FR-016 | Phase 7/8 | `task.start` contract test with `query|goal|task`, optional package, MCP smoke test, and Windows-first on-demand startup |
| FR-017 | Phase 7 | append-only `lore_feedback`, three outcomes, provenance isolation, and `scores.feedback` in Retrieval; `tests/context.rs` |
| FR-018 | Phase 1/6/7 | CLI contract tests and smoke (`search`, `recall`) plus MCP (`task.start`, `feedback`) |
| FR-019 | Phase 5 | `lore session delete` and `lore knowledge delete` remove derived data transactionally |
| FR-020 | Phase 2/8 | hook installation, composition, removal, and restoration |
| FR-021 | Phase 8.1 | `setup --check|--apply|--remove`, Codex/Claude/Gemini adapters, ownership, and generic snippet; `tests/phase81_windows.rs` |

## Non-functional requirements

| ID | Owning phase | Planned evidence/test |
|---|---|---|
| NFR-001 | Phase 1/8 | local journey without an external service |
| NFR-002 | Phase 0/5 | privacy matrix, redaction before FTS, and JWT/secret test |
| NFR-003 | Phase 2/8 | commit/editing continue when Lore is unavailable |
| NFR-004 | Phase 2/4 | durable inbox, restart, claim/retry, and dead-letter |
| NFR-005 | Phase 1/3 | port tests and connector without infrastructure imports |
| NFR-006 | Phase 1/5/6 | migrations v1–v5 and promotion, reindex, and cross-version recovery fixtures |
| NFR-007 | Phase 1/8 | structured logs without sensitive payloads |
| NFR-008 | Phase 6 | `lexical_only`/absent provider keeps FTS5 recall functional |
| NFR-009 | Phase 8 | Windows-first validated; Linux/macOS remain residual risk and are not declared supported |
| NFR-010 | Phase 2/8 | `tests/phase8_windows.rs`: uninstall removes hooks, cooperative shutdown releases the lock, and data remains by default |
| NFR-011 | Phase 6/7 | provenance, scores, and `why_selected` in every result; context package includes provenance/confidence/reason and is validated in `tests/context.rs` |
| NFR-012 | Phase 7 | `authority=non_authoritative_context`, serialization test, and MCP contract prevent precedence over current instructions |
| NFR-013 | Phase 8.1 | read-only check, safe backup/write, idempotency, rollback, `core.hooksPath`, and MCP handshake; `tests/phase81_windows.rs` |

## Traceability rules

- `pending` means the phase has not provided evidence yet; it does not mean the requirement was rejected.
- A unit test does not replace integration testing when a requirement involves SQLite, hooks, the watcher, MCP, or a real process.
- Any change to the owning phase must update this matrix and the dossier in the same change.
- The Phase 0 gate considers the matrix complete when every ID has an owning phase and planned evidence; it does not declare future requirements implemented.
- Phase 8 evidence is explicitly Windows-first; lack of Linux/macOS evidence must not be interpreted as multiplatform approval.
