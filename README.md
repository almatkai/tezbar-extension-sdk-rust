# Tezbar Rust extension SDK

An independent Rust crate implementing frozen Tezbar external process protocol 1.
See [SDK_API.md](SDK_API.md) for the adapter API, exact limits and lifecycle ownership.
No Tezbar source checkout, Tauri, Bun or app configuration is needed to build/run.

```sh
CARGO_BUILD_JOBS=2 cargo build --release --locked
CARGO_BUILD_JOBS=2 cargo test --locked -- --test-threads=1
cargo run --bin sdk-fixture -- --protocol-info
```

The fixture executable is for protocol tests, not a shipped extension. Integration
tests spawn actual child processes with pipes, reply deadlines and kill-on-drop.
Only stdout JSONL is permitted. Diagnostics go to stderr. Errors in framing are
fatal; host restarts on a later explicit action and must never replay side effects.

Each process owns one extension/profile. Host must verify installed identity,
version, capabilities and artifact hashes, provide private scoped paths, keep the
pipe reader independent of reverse calls, and enforce process-tree/UI cleanup on
hard termination. Native binaries are not OS-sandboxed by capability grants.

Port developers: register asynchronous work with `ctx.spawn`; background work
requires an acknowledged host lease and lifecycle hook ownership. Do not leave
untracked threads/helpers running. Use kill-on-drop for helper subprocesses and
explicitly stop thread-affine actors in `shutdown`. Host kills descendants on
crash or grace expiry. Durable leases survive host shutdown by host policy.

Local ports may use a sibling path dependency during development. Published
extensions must pin an immutable SDK git revision, commit Cargo.lock, and pass
isolated CI builds. See [PROTOCOL.md](PROTOCOL.md) for the wire contract.
The workflow checks this SDK on native macOS ARM and Windows x64; workflow presence
does not claim those jobs have run.
