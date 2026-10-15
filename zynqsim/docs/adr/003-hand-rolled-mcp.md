# ADR-003: Implement MCP over stdio directly

Status: **Accepted**. Date: 2026-10-15.

## Context

The MCP server must expose 39 tools (the 38 of §13 plus `trace_get`), resources and prompts with strict schemas, confine file
access, bound outputs and support progress and cancellation (prompt §13). It needs to be Rust.

## Options

1. A Rust MCP SDK crate.
2. A direct JSON-RPC 2.0 implementation on `serde_json`.

## Decision

Option 2. The protocol surface used is small (`initialize`, `ping`, `tools/*`, `resources/*`,
`prompts/*`, two notifications), newline-delimited over stdio. Owning it means:

* the tool table, argument validation and gating are ours and unit-testable in-process;
* cancellation needs to interleave reads with a running tool call, which a reader thread and a
  `Control` callback make explicit;
* no dependency churn from a fast-moving SDK, and no async runtime in the workspace.

## Consequences

* We track the spec ourselves. `initialize` negotiates among `2025-06-18`, `2025-03-26` and
  `2024-11-05`; unknown versions are answered with the newest we speak.
* Batch requests are rejected (removed from the spec in 2025-06-18).
* The streamable HTTP transport is not implemented (STATUS: stub). The `McpServer::handle` core
  is transport-independent, so adding it does not touch the tools.

## Exit plan

`McpServer::handle(Value, &mut dyn Control) -> Option<Value>` is the only seam. Replacing the
transport or adopting an SDK means re-implementing `stdio.rs`, not the tool logic.
