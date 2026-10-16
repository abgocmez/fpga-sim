# cargo-fuzz targets

Coverage-guided fuzzing for the project parser (`project_parse`), the MCP request handler
(`mcp_request`) and the remote-port packet decoder (`interop_packet`). Status: **implemented-untested**
(written, type-checked against a stub; never run under libFuzzer).

The seeded, deterministic versions of the same checks run in the normal test suite on stable Rust:
`crates/sim-api/tests/fuzz_project.rs`, `crates/mcp/tests/fuzz_requests.rs`, `crates/interop/tests/fuzz_packets.rs`.

Run on Linux with a nightly toolchain and `cargo-fuzz`:

    cargo install cargo-fuzz
    cd zynqsim/fuzz
    cargo +nightly fuzz run project_parse
    cargo +nightly fuzz run mcp_request
    cargo +nightly fuzz run interop_packet

Corpora and crashes go to `fuzz/corpus/` and `fuzz/artifacts/` (git-ignored). This directory is not a
workspace member; `cargo build --workspace` does not build it.
