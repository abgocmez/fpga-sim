# ADR-005: FNV-1a-64 over a canonical encoding as the trace digest

Status: **Accepted**. Date: 2026-10-15.

## Context

Determinism tests (prompt §0.5, §14.6) compare "trace hashes". The digest must be stable
across releases, independent of serialisation details, and trivial to reproduce elsewhere (for
example from a Python or C test harness reading a recording).

## Decision

64-bit FNV-1a over a hand-specified byte encoding of each `(time, event)` record:
`time` as 8 little-endian bytes, then a tag byte (1 input, 2 signal, 3 UART, 4 marker) and the
fields, with strings length-prefixed (8-byte LE length). Rendered as `fnv1a64:<16 hex digits>`.

The encoding is deliberately *not* serde output, so renaming a JSON field cannot silently change
every stored digest.

## Consequences

* It detects divergence; it is **not** collision-resistant and must not be used for integrity
  or security. 64 bits is ample for comparing two runs of the same design.
* Changing the encoding invalidates stored digests; if it ever must change, bump
  `Recording.version` so old recordings are rejected rather than reported as mismatches.
* Reference vectors for FNV-1a are tested (`trace::hash::tests::matches_published_fnv1a_vectors`).
