# CompletionPolicy and ConfidencePolicy

**Status:** Baseline implemented and validated in Phase 4 on Windows-first
**Central rule:** completing a session does not mean promoting knowledge.

## 1. Session states

| State | Entry | Permitted exit |
|---|---|---|
| `open` | first task event | `completed`, `rejected`, or `expired` |
| `completed` | explicit success or an approved combination | terminal; may produce a candidate |
| `rejected` | explicit failure/correction | terminal; does not produce an eligible candidate |
| `expired` | TTL elapsed without sufficient evidence | terminal; any candidate remains ineligible |

Branch changes, topic changes, and an isolated `AgentStopped` do not complete a session. A connector without lifecycle support must declare the missing capability and let the core use TTL and local evidence.

## 2. CompletionPolicy

| Case | Events | State | Reason |
|---|---|---|---|
| C-001 | `TaskFinished { outcome: success }` | `completed` | explicit completion |
| C-002 | `AfterTask { status: success }` + tests passed | `completed` | reinforced agent completion |
| C-003 | `TaskFinished { outcome: failed }` | `rejected` | explicit failure |
| C-004 | `AgentStopped` without success or error | `open` until TTL, then `expired` | absence is not success |
| C-005 | `BranchChanged` during a task | remains `open` | branch change is evidence, not completion |
| C-006 | commit without a task event | provisional session may be `completed` only with a link and test/result; otherwise `open` | a commit alone does not prove intent |
| C-007 | out-of-order event | reorder by `occurred_at` within the window; record a diagnostic outside the window | do not corrupt state |
| C-008 | user corrects a solution after success | previous session remains `completed`; a new session/candidate records the correction | do not rewrite provenance |

The initial TTL is configurable and must be measured with real fixtures before it is frozen. The implementation must be deterministic: the same ordered event set produces the same state.

## 3. ConfidencePolicy

The initial score is `0..=100`, explained by signals and separate from completion. The initial promotion threshold is `60`; the table was validated as the Phase 4 operational baseline and can be recalibrated with future real cases.

| Signal | Weight | Note |
|---|---:|---|
| `user_accepted` | +25 | explicit user confirmation |
| `tests_passed` | +25 | relevant tests passed |
| `commit_created` | +20 | change was recorded in the project |
| `reused_successfully` | +20 | knowledge worked in a later task |
| `user_corrected` | -25 | solution required correction |
| `tests_failed` | -25 | evidence of failure |
| `rejected_explicitly` | -40 | user rejected the candidate |

The score is bounded to `0..=100`, records every applied signal, and never uses a topic change as a strong positive signal. A completed candidate below the threshold remains stored only as a transient candidate and does not appear in recall.

## 4. Tabular cases

| Case | Evidence | Completion | Expected score | Promotion |
|---|---|---|---:|---|
| S-001 | task accepted, tests passed, commit | `completed` | 70 | yes |
| S-002 | task accepted, tests passed, commit, reuse | `completed` | 90 | yes |
| S-003 | task accepted, tests failed | `completed` | 0 | no |
| S-004 | agent stopped without a result | `expired` | 0 | no |
| S-005 | solution corrected by the user | `rejected`/new session | 0 or new score | do not promote the old corrected version |
| S-006 | topic change only | `open` | 0 | no |
| S-007 | commit without tests or acceptance | provisional `completed` or `open`, depending on the link | 20 | no |
| S-008 | candidate reused successfully | previous terminal state preserved | +20 to new calculation | reevaluate, never mutate provenance |

## 5. Testable invariants

1. `completed` does not imply `eligible_for_promotion`.
2. A duplicate event does not change the score or transition.
3. Events from different sessions cannot mix signals.
4. Every promotion points to the source session, candidate, and signals.
5. A correction creates a new version/candidate; it does not delete prior evidence.
6. Score and explanation are reproducible without calling an LLM.

## 6. Implemented operational baseline

- Initial session TTL: 24 hours without completion evidence.
- Reordering diagnostic window: 5 minutes.
- Inbox worker: batches of up to 32 events and a maximum of 3 attempts per event.
- `processing`, `processed`, and `dead_letter` states are recoverable and observable in the SQLite inbox.
- Candidates are persisted separately with signals, score, provenance, and `eligible_for_promotion`; Phase 5 automatically promotes only eligible candidates and keeps the source-candidate trail.
- Extraction uses only structured fields from `metadata_only` events and does not require an LLM.
