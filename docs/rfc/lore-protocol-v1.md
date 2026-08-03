# RFC-0001 — Lore Protocol v1

**Status:** Accepted for implementation planning
**Version:** 1
**Date:** 2026-08-02
**Scope:** neutral contracts between agents, connectors, and the Lore local core

## 1. Purpose and limits

Lore Protocol is the stable contract that lets an agent send evidence, request context, and record feedback without knowing about SQLite, the Rust runtime, or any AI model. MCP, the CLI, and future adapters translate the transport; capture, learning, persistence, and retrieval rules remain in the core.

This version does not define a UI, remote synchronization, authentication, an embedding model, or a raw chat format. The product remains local-first, and prompt/response content is not persisted by default.

## 2. Compatibility rules

1. `protocol_version` is an integer. The value `1` identifies this RFC.
2. Required fields must not be removed or have their meaning changed within version 1.
3. New optional fields and unknown properties are ignored by consumers that do not recognize them.
4. A consumer must reject a version greater than the supported version with `unsupported_protocol_version`; it must not interpret the payload as if it were v1.
5. `event_id` is the idempotency key. Resending the same envelope does not duplicate effects.
6. The envelope accepts event types registered in this RFC and non-empty extensions; extensions are preserved in the inbox but do not enter the Learning Engine until a corresponding capability/policy exists.
7. Breaking changes require `protocol_version = 2` and a new compatibility document.

## 3. Conventions

- Times are Unix epoch seconds in UTC (`u64` in the current core).
- IDs are opaque strings that remain stable during an object's lifecycle. Connectors must limit external inputs to 128 characters; Phase 2 Capture validates non-empty values, while size validation belongs to the protocol adapter.
- Local project identifiers use `local-<sha256-prefix>` until remote-based identity is approved.
- Official event names use PascalCase to remain compatible with the preparation specification examples.
- JSON is UTF-8; unknown objects inside `payload` are preserved by Capture and interpreted only by the component that knows the type.

## 4. EventEnvelope

### 4.1 Contract

| Field | Type | Required | Rule |
|---|---|---:|---|
| `protocol_version` | integer | yes | must be `1` |
| `event_id` | string | yes | non-empty; unique per logical event |
| `session_id` | string | yes | non-empty; may be provisional in Capture |
| `project_id` | string | yes | non-empty; project scope |
| `source` | string | yes | non-empty; e.g. `mcp`, `git_hook`, `filesystem`, `cli` |
| `event_type` | string | yes | non-empty; official registry or extension |
| `occurred_at` | integer | yes | UTC epoch seconds |
| `privacy_mode` | enum | yes | `metadata_only`, `redacted`, or `content_opt_in` |
| `payload` | JSON value | yes | object recommended; no deliberately persisted secret |

`metadata_only` permits only metadata needed for correlation, such as event type, commit, branch, and relative paths. `redacted` is valid only after the redactor removes sensitive content. `content_opt_in` requires explicit consent recorded for the project and never turns raw chat into a Knowledge Unit.

### 4.2 Initial event registry

`BeforeTask`, `AfterTask`, `TaskAccepted`, `ConversationEnded`, `PromptSent`, `ResponseReceived`, `FilesChanged`, `CommitCreated`, `BranchChanged`, `TestsExecuted`, `ToolCalled`, `AgentStopped`, `TaskFinished`, and `HookExecuted` are official names. A connector may send `x.<connector>.<name>` as an extension; the core must store and diagnose the extension without inferring meaning.

### 4.3 Valid example

```json
{
  "protocol_version": 1,
  "event_id": "01J9MCP7F0C8E8WQ7K7E5W4Q2V",
  "session_id": "session-2026-08-02-001",
  "project_id": "local-a1b2c3d4e5f6",
  "source": "filesystem",
  "event_type": "FilesChanged",
  "occurred_at": 1754146800,
  "privacy_mode": "metadata_only",
  "payload": {
    "paths": ["src/auth.rs"],
    "kinds": ["Modify"]
  }
}
```

### 4.4 Invalid examples

Unsupported version:

```json
{ "protocol_version": 2, "event_id": "evt-1", "session_id": "s-1", "project_id": "p-1", "source": "mcp", "event_type": "BeforeTask", "occurred_at": 1754146800, "privacy_mode": "metadata_only", "payload": {} }
```

Missing required field:

```json
{ "protocol_version": 1, "event_id": "", "session_id": "s-1", "project_id": "p-1", "source": "mcp", "event_type": "BeforeTask", "occurred_at": 1754146800, "privacy_mode": "metadata_only", "payload": {} }
```

Privacy violation:

```json
{ "protocol_version": 1, "event_id": "evt-2", "session_id": "s-1", "project_id": "p-1", "source": "mcp", "event_type": "PromptSent", "occurred_at": 1754146800, "privacy_mode": "metadata_only", "payload": { "raw_prompt": "token=secret-value" } }
```

## 5. LearningSession

A session groups events related to one task. Capture may create a provisional session; only CompletionPolicy may close it.

```json
{
  "session_id": "session-2026-08-02-001",
  "project_id": "local-a1b2c3d4e5f6",
  "state": "completed",
  "started_at": 1754146700,
  "ended_at": 1754146800,
  "event_count": 8,
  "completion_reason": "TaskFinished.success"
}
```

Allowed states are `open`, `completed`, `rejected`, and `expired`. `open` moves to `completed` only with explicit evidence or an approved signal combination; explicit failure moves to `rejected`; silence beyond the TTL moves to `expired`. `BranchChanged` or a topic change alone does not close a session.

Invalid example:

```json
{ "session_id": "session-1", "project_id": "p-1", "state": "completed", "started_at": 1754146700, "event_count": -1 }
```

The example violates `ended_at`/`completion_reason` and uses a negative `event_count`.

## 6. CandidateKnowledge

`CandidateKnowledge` is the Learning Engine's provisional result. It is not yet retrievable.

```json
{
  "candidate_id": "candidate-001",
  "session_id": "session-2026-08-02-001",
  "goal": "Validate JWT tokens in middleware",
  "context": "Local API with expiration and refresh tokens",
  "constraints": ["no external service"],
  "solution": "Centralize validation in middleware and cover expiration in tests",
  "artifacts": ["src/auth.rs", "tests/auth.rs"],
  "decision_summary": "The solution was kept after the tests passed",
  "confidence_score": 80,
  "signals": ["tests_passed", "commit_created"],
  "eligible_for_promotion": true,
  "created_at": 1754146800
}
```

`decision_summary` is an objective, auditable justification; chain-of-thought is never stored. A candidate without a `goal`, `solution`, provenance, or explainable score is invalid and cannot be promoted.

```json
{ "candidate_id": "candidate-002", "session_id": "session-1", "goal": "", "solution": "", "confidence_score": 120, "eligible_for_promotion": true }
```

## 7. KnowledgeUnit

`KnowledgeUnit` is the only retrievable knowledge object. It must be structured, versioned, and traceable.

```json
{
  "id": "knowledge-001",
  "version": 1,
  "goal": "Validate JWT tokens in middleware",
  "context": "Local API with expiration and refresh tokens",
  "constraints": ["no external service"],
  "solution": "Centralize validation in middleware and cover expiration in tests",
  "artifacts": ["src/auth.rs", "tests/auth.rs"],
  "decision_summary": "Promoted after a commit and successful tests",
  "confidence": 80,
  "scope": "project",
  "project_id": "local-a1b2c3d4e5f6",
  "created_at": 1754146800,
  "updated_at": 1754146800,
  "related_ids": [],
  "provenance": { "session_id": "session-2026-08-02-001", "candidate_id": "candidate-001" }
}
```

A unit without `provenance`, with raw chat in any field, or with `confidence` outside `0..=100` is invalid. Promotion is transactional and idempotent.

```json
{ "id": "knowledge-002", "version": 1, "goal": "store conversation", "solution": "full transcript", "confidence": 101, "scope": "global" }
```

## 8. RecallRequest and ContextPackage

### 8.1 RecallRequest

```json
{
  "request_id": "recall-001",
  "project_id": "local-a1b2c3d4e5f6",
  "session_id": "session-2026-08-02-002",
  "query": "How did we validate JWTs in this project?",
  "scope": "project_then_global",
  "budget": 5,
  "capabilities": ["lexical", "semantic", "structured_filters"]
}
```

`query` must not be empty; `budget` must be positive and bounded by Context Builder; `scope` is `project`, `global`, or `project_then_global`. Recall does not change Knowledge Units.

Invalid example:

```json
{ "request_id": "recall-002", "project_id": "", "query": "", "scope": "cloud", "budget": 0 }
```

### 8.2 ContextPackage

```json
{
  "package_id": "context-001",
  "request_id": "recall-001",
  "authority": "non_authoritative_context",
  "entries": [
    {
      "knowledge_id": "knowledge-001",
      "version": 1,
      "summary": "Centralize JWT validation in middleware and cover expiration",
      "confidence": 80,
      "why_selected": ["project_match", "lexical_match", "tests_passed"],
      "origin": { "project_id": "local-a1b2c3d4e5f6", "session_id": "session-2026-08-02-001" }
    }
  ],
  "budget_used": 1,
  "generated_at": 1754146800
}
```

The package never presents itself as a higher-priority instruction than the user's current message. Duplicates and superseded versions must not consume the budget twice. A package without `authority`, `why_selected`, or provenance is invalid. The builder uses `non_authoritative_context`, prioritizes the current project, limits the default budget to 5 (maximum 20), and may be returned automatically by `task.start` when metadata contains `query`, `goal`, or `task`.

Valid feedback preserves the unit and records only observed usage:

```json
{
  "operation": "feedback",
  "project_id": "local-a1b2c3d4e5f6",
  "knowledge_id": "knowledge-001",
  "version": 1,
  "session_id": "session-2026-08-02-003",
  "outcome": "used",
  "note": "applied while validating the middleware"
}
```

`ignored` and `corrected` use the same format. The record does not alter `origin`, `content_hash`, or the Knowledge Unit version.

## 9. Connector handshake and operations

Codex is the first homologation client, without coupling the protocol to the Codex product.

```json
{
  "operation": "handshake",
  "protocol_version": 1,
  "client_id": "codex",
  "client_version": "dev",
  "capabilities": ["event_ingest", "task_lifecycle", "recall", "feedback"]
}
```

Response:

```json
{
  "protocol_version": 1,
  "accepted": true,
  "server_version": "0.1",
  "capabilities": ["event_ingest", "task_lifecycle"],
  "automation_level": "capture_and_lifecycle"
}
```

Invalid handshake example:

```json
{ "operation": "handshake", "protocol_version": 2, "client_id": "", "capabilities": "all" }
```

Version 1 operations are `handshake`, `event.ingest`, `task.start`, `task.end`, `recall`, and `feedback`. The server announces `event_ingest` and `task_lifecycle`; when Retrieval and Knowledge Store are composed, it announces `recall` and `feedback`. `feedback` accepts `used`, `ignored`, and `corrected`, with optional `version` and `session_id`, and is persisted as append-only usage without changing provenance. A client without `task_lifecycle` may still send events; the server must report degradation and never pretend to provide full automation.

Normative errors:

| Code | Use |
|---|---|
| `invalid_envelope` | missing, empty, or invalid-type field |
| `unsupported_protocol_version` | version greater than supported |
| `duplicate_event` | event already persisted; response is idempotent and does not fail the flow |
| `privacy_violation` | payload incompatible with the privacy mode |
| `capability_unavailable` | operation unsupported by the connector/runtime |
| `storage_unavailable` | inbox or database unavailable |
| `internal_error` | unclassifiable error; no sensitive details |

## 10. Version evolution

Version 1.1 may add optional properties such as `trace_id`, `redaction_profile`, or new official types without changing the meaning of existing fields. The v1 core must ignore unknown properties but preserve the received envelope for diagnostics. A type change, required-field removal, privacy-semantics change, or new terminal state requires v2, a migration, and cross-version contract tests.

## 11. Acceptance criteria

- The schemas above have valid and invalid examples for every contract.
- Idempotency by `event_id` is observable in the inbox.
- CompletionPolicy and ConfidencePolicy are separate and follow the matrix in [`completion-confidence.md`](../policies/completion-confidence.md).
- The requirements matrix in [`requirements-traceability.md`](../requirements-traceability.md) links every FR/NFR to a phase and planned test.
- No protocol rule requires MCP, SQLite, or a specific client.

### Implementation maturity note

Capture validates version and non-empty fields, persists the envelope, and deduplicates `event_id`. The MCP connector validates `metadata_only`; the Knowledge Store applies redaction at the promotion boundary, persists only structured fields, and maintains transient retention. Phases 6 and 7 implement `recall`, Context Builder, auto-recall, and feedback with FTS5, an optional local provider, filters, explainable ranking, and a reuse signal. This RFC freezes the wire contract without promoting retrieved knowledge to an authoritative instruction.
