# ADR-0003 — Ports and Adapters in a modular crate

**Status:** Accepted
**Date:** 2026-08-02

## Context

The core must not depend directly on SQLite, MCP, the filesystem, or an embedding provider, but multiple crates and a dependency-injection container would add complexity without a second consumer.

## Decision

The single package is organized into `domain`, `application`, `infrastructure`, and `interfaces`. Ports live in the application layer; concrete adapters live in infrastructure; the CLI/runtime is the composition root. Repositories, Strategy, Adapter, explicit states, local Observer, and Transactional Inbox are allowed when they have a clear responsibility. Full CQRS, Event Sourcing, a generic Mediator, and abstractions without a second use are prohibited until evidence justifies them.

## Consequences

- Policies and entities can be tested without a database or transport.
- Adapters can be swapped in contract/integration tests.
- Splitting into separate crates requires proven pressure from compilation, ownership, distribution, or reuse.

## Validation

Import reviews, port tests, `clippy -D warnings`, and contract tests verify that business rules do not import infrastructure.
