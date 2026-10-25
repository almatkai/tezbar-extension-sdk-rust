# SDK validation

Source evidence: frozen app commit fe71ca87f21ac4ff31505010c228f8abda0f2a89.

Local environment: macOS 26.3 (25D2125), Darwin arm64,
Rust 1.98.0 / Cargo 1.98.0. macOS ARM protocol/process tests ran locally.
Windows x64 MSVC build/tests and GitHub workflow execution have NOT run.

Commands passed with CARGO_BUILD_JOBS=2:

- `cargo test --offline --locked -- --test-threads=1`: **16 tests passed**
  (4 helpers + 12 process integration; 0 ignored).
- `cargo build --release --offline --locked`.
- `cargo clippy --offline --locked --all-targets -- -D warnings`.
- `cargo fmt --check`.

Actual process coverage: hello/initialize identity; fragmented UTF-8 and a partial
frame surviving concurrent handler completion; independent two-way reverse RPC;
refresh under contention; cancellation emitting reverse cancel; late responses;
stale-search transactional state; two-session isolation; dispose/task fencing;
durable retain acknowledgement and background/tray-event-shaped payload routing;
revisioned invalidation; shutdown while host stdin remains open; malformed/CRLF/truncated/oversize input; oversize handler
output; EOF with reverse RPC pending; 128-session limit; duplicate active IDs;
64-task scope cap/recovery; 120-action capacity plus reserved shutdown control slots.
The in-process drop test verifies owned future destruction and writer EOF.

Logs: /tmp/tezbar-external-rust/sdk-test.log, sdk-build.log, sdk-clippy.log,
sdk-format.log. An intermediate capacity-test expectation failure was retained at
sdk-test-capacity-adjustment.log; final sdk-test.log is green after matching the
new control-slot reservation and verifying saturated shutdown.

API: [SDK_API.md](SDK_API.md).
Exact host work: HOST_INTEGRATION.md. The crate contains no app/Tauri dependency.
Generic migration helpers beyond Perplexity need their own port + real host tests;
SDK fixture success does not establish real OS clipboard/tray/notifications behavior.
Host owns executable verification, process-tree kill, host resource cleanup,
generation fencing, persistent lease registration/restore and renderer translation.
Port owns cleanup of unregistered threads/native helpers and durable domain state.

Extension publication requires an immutable SDK revision, updated lockfiles,
and native isolated release CI. This local validation does not establish Windows
or live OS integration behavior.
