# ADR-0004 — MCP as the first official connector

**Status:** Accepted
**Date:** 2026-08-02

## Context

MCP is a useful port for the first agent, but it must not become Lore's domain. Clients vary in lifecycle capabilities, and universal event coverage cannot be assumed.

## Decision

MCP is the first official connector and Codex is the first homologation client. The connector translates handshake, events, recall, and feedback to Lore Protocol v1. It does not access SQLite, calculate confidence, or decide promotion. Capabilities are negotiated; a missing capability lowers the automation level reported to the client.

## Consequences

- The same use case must work through the test CLI and MCP.
- Claude, Cursor, VS Code, and other integrations reuse the protocol instead of copying rules.
- Until MCP exists, hooks, the watcher, and the CLI exercise the envelope and local Capture.

## Validation

Handshake contract tests, unknown-version handling, extra fields, missing capabilities, and the Codex journey `task.start → events → recall → feedback` form the Phase 3 gate.
