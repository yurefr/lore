# Initial privacy and retention policy

**Status:** Accepted for MVP planning
**Applies to:** Capture, Learning Engine, Knowledge Store, Retrieval, and connectors

## 1. Principles

1. Lore is local-first: no external server is required to capture, learn, or retrieve.
2. The default mode is `metadata_only`.
3. Raw prompts, responses, and tool calls are not Knowledge Units and are not persisted by default.
4. Lore does not deliberately persist tokens, cookies, private keys, passwords, authorization headers, or credentials.
5. Retrieved context is non-authoritative; current user instructions and agent policies take precedence.
6. Any mode that permits content requires explicit consent, redaction before persistence, and shorter retention than Knowledge Units.

## 2. Data matrix

| Source/field | Default mode | Purpose | Persistence | Deletion |
|---|---|---|---|---|
| `project_id`, normalized path, and project name | metadata | scope, diagnostics, and filters | while the project is registered | project deletion will be added with the project lifecycle |
| event type, source, timestamps, and IDs | metadata | correlation, idempotency, and auditing | inbox until processing; provenance metadata while a Knowledge Unit exists | session/unit deletion |
| branch, commit, test, and relative paths | metadata | solution evidence and ranking | inbox until processing; Knowledge Unit provenance | session/unit deletion |
| prompt/response/tool call | not collected | none in the default mode | not persisted | not applicable |
| redacted opt-in content | `redacted`/`content_opt_in` | candidate extraction with consent | at most until consolidation + 7 days | immediate user deletion |
| `CandidateKnowledge` | structured | review/promotion | until promotion, rejection, or transient retention | session deletion |
| `KnowledgeUnit` | structured | recall and reuse | until user deletion or a future retention policy | `lore knowledge delete` |
| usage feedback (`used`, `ignored`, `corrected`) | structured | ranking recalibration and reuse auditing | while the Knowledge Unit exists | unit deletion through FK |
| stderr logs | no payload | operational diagnostics | not persisted by the core | cleanup by the host process |

Initial transient retention: `pending`, `processing`, and `dead_letter` events remain until processing or for at most 30 days; the Phase 4 worker records the decision. Opt-in content, when enabled, expires seven days after session closure unless it was promoted to a structured summary without the raw content.

## 3. Redaction and deletion rules

The Phase 5 redactor removes or replaces the following before persisting a Knowledge Unit:

- token, API key, JWT, cookie, password, and `Authorization` header patterns;
- private-key material (`BEGIN ... PRIVATE KEY`), private certificates, and credential files;
- values of environment variables marked as secrets;
- raw content that cannot be reduced to `goal`, `context`, `constraints`, `solution`, `artifacts`, and `decision_summary`.

In `metadata_only`, the watcher records only relative paths, event types, and event metadata; it does not open files to read content. The sensitive filename policy for any future opt-in capture remains separate from the promotion boundary: the official connector still rejects `content_opt_in`, and the Knowledge Store redacts structured fields when an authorized fixture contains a secret.

## 4. Consent and controls

- `metadata_only` is enabled at initialization, with an option to pause capture per project.
- `redacted` and `content_opt_in` require explicit configuration and must appear in status/diagnostics.
- The user can delete a session, candidate, or Knowledge Unit; deletion removes derived relations or marks them orphaned deterministically.
- `lore knowledge delete` removes a unit, its versions, relations, and usage history in one transaction; `lore session delete` removes the session, candidates, events, and derived units.
- `lore knowledge cleanup` applies retention to terminal events and transient candidates without touching promoted units.
- Feedback does not replace or rewrite `KnowledgeUnit`, provenance, or content; the optional note is limited to 512 characters and the record remains separate.
- A consent failure must degrade to `metadata_only` or reject the event with `privacy_violation`; content must never be captured silently.
- The user can remove hooks and disable the watcher without leaving a required process or configuration behind.

## 5. Local model and data outside the database

The embedding provider is local and optional. The `lore-hash-v1` baseline is embedded, deterministic, and does not download anything. Future model-backed providers may download models only with explicit consent, a checksum, and a displayed license; there is no silent automatic download. Without a provider, Retrieval uses FTS5 and structured filters. No prompt or Knowledge Unit is sent to a remote provider as part of normal operation.

## 6. Acceptance cases

| Case | Input | Expected result |
|---|---|---|
| P-001 | `FilesChanged` event in `metadata_only` | persist relative paths and types without opening the file |
| P-002 | `PromptSent` with `raw_prompt` in `metadata_only` | reject or reduce to metadata; never store the text |
| P-003 | opt-in content containing a JWT | redactor replaces the secret before inbox/candidate persistence |
| P-004 | connector without content consent | handshake reports the missing capability and capture remains metadata-only |
| P-005 | user deletes a Knowledge Unit | unit, derived versions, and relations are removed transactionally |
| P-006 | local model unavailable | lexical recall continues to work; no network is required |

## 7. Deferred decisions that do not block the baseline

The exact Knowledge Unit retention period, stable cross-machine identity, and final sensitive filename pattern list remain outside the baseline. Phase 5 fixes only the operational transient retention: metadata-only inbox for 30 days, authorized content for 7 days, and rejected/expired candidates for 30 days. Changes must produce an ADR and migration when they affect existing data.

## 8. Phase 5 implementation evidence

- `KnowledgeUnit` is built only from an eligible, completed candidate with provenance and minimum fields.
- Promotion writes the unit, FTS, declared relations, and candidate promotion marker in one SQLite transaction; repeating the same candidate or fingerprint does not duplicate the unit.
- FTS receives only already-redacted structured fields. Raw prompts and responses do not enter the knowledge schema.
