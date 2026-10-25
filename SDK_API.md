# Rust extension SDK API — protocol 1, crate 0.1.1

Independent crate `tezbar-extension-sdk`, imported as `tezbar_extension_sdk`.
No app/Tauri dependency. Local ports may use `path = "../tezbar-extension-sdk-rust"`
until parent publishes and pins an immutable git revision. Commit each lockfile.

```rust
use tezbar_extension_sdk::{Extension, ExtensionContext, CoreResult, BoxFuture};
use serde_json::Value;
struct Adapter;
impl Extension for Adapter {
    fn id(&self) -> &'static str { "com.tezbar.native.example" }
    fn version(&self) -> &'static str { "1.0.0" }
    fn commands(&self) -> &'static [&'static str] { &["command"] }
    fn handle<'a>(&'a self, ctx: &'a ExtensionContext, command: &'a str,
        operation: &'a str, payload: &'a Value, state: &'a mut Value)
        -> BoxFuture<'a, CoreResult> { Box::pin(async move { Ok(state.clone()) }) }
}
// #[tokio::main(worker_threads = 2)] async fn main() { serve(Adapter).await.unwrap(); }
```

`serve(adapter)` uses stdin/stdout JSONL, emits hello first, supports
`--protocol-info` and `--self-test` without host/GUI. `serve_io` is the same runner
with generic asynchronous pipes. The stdio entrypoint owns one bounded stdin pump
thread (2 × 8 KiB chunks); it ends on EOF or process exit and cannot hold Tokio
runtime shutdown open while the host pipe remains open. `CoreResult = Result<Value,String>`.
`Extension` also supplies default `initialize`, `background_activate`,
`background_event`, `shutdown` hooks returning `BoxFuture<CoreResult>`.
Background callbacks receive the frozen request params unchanged.

`ExtensionContext` is cloneable: `paths.package_dir`, `paths.data_dir`,
`paths.temp_dir`, `platform`, `migration_paths` (explicit host grants only),
`granted_capabilities`, `host_client`. `ctx.host(method, params).await` sends typed
`host.*` reverse RPC. `ctx.request(channel, Vec<Value>).await` is a strict migration
allowlist (shell/clipboard/selection/window/screenshot/dialog/AI/settings/safety);
unknown channels and all named-port/process-domain channels are errors. Prefer
`ctx.host("host.shell.open", json!({"target": url})).await` for new code.
Capability grants accept exact `host.shell.open`, `shell.open`, or group `shell`;
host remains authoritative. Storage is local and requires no reverse RPC.

`ctx.emit(event, data)` emits generic events; `ctx.invalidate()` emits
`session.invalidated` with canonical sessionId and monotonically increasing revision.
`ctx.spawn(future)` registers bounded session/background work (max 64 per scope);
`ctx.cancelled().await` observes cancellation. Session contexts reject events after
dispose; operation cancellation suppresses late invalidations. `ctx.spawn` tasks
belong to the session, and finish/abort on dispose. Use the background hook context
for explicit process-owned work, after acknowledged host.background.retain.

`host.background.retain {key,durable,data?}` must be awaited before acknowledging
durable state changes; `host.background.release {key}` releases it. Tray and
notification calls use frozen `host.tray.set/remove`, `host.notifications.show/remove`.
Handlers own domain resource bookkeeping and release resources explicitly. The SDK
cancels tracked futures on EOF/protocol error/shutdown; it cannot stop arbitrary
unregistered threads or `std::process::Child`. Wrap helpers with kill-on-drop,
register async tasks via ctx.spawn, and stop actor threads in shutdown. Host MUST
kill the process tree on crash/hard timeout and clean host UI resources/leases.
Shutdown hook runs after session cancellation, bounded to 2 seconds. No detached
`tokio::spawn` in ports unless owned and explicitly stopped by lifecycle code.

`Extension::concurrency(command)` defaults to `CommandConcurrency::Serialize`;
return `RejectWhileBusy` to reject overlapping action handlers without side effects.
Mirror this in the manifest command `concurrency` field.

Session open calls adapter operation `extension:run-command`. Calls augment payload
with canonical sessionId/preferences/argumentValues. Dispose cancels running work,
then invokes `extension:dispose-session` with bounded cleanup. State starts `{}`;
operations serialize. Search `extension:search-text-changed` supersedes the prior
search at reader dispatch (including queued searches), operates on cloned state,
and commits only if current. All calls stage state and commit on success; side
effects/storage are not rolled back. `extension:load-more` is also accepted.
Busy refresh returns `unchanged()` immediately.
No wire protocol changes. Limits: 16 MiB/frame, 128 sessions, 128 handlers
(120 ordinary + 8 cleanup reserve), 128 pending reverse RPC, 256 output frames,
64 scoped tasks. Saturation returns errors;
reverse RPC timeout 120 seconds. Cancel uses original requestId; dropping a pending
reverse future removes its waiter and emits cancel to the host. EOF drains waiters.

Helpers: `view(title,root,actions)`, `no_view(message)`, `unchanged()`,
`open(ctx,target).await`, `copy(ctx,text).await`, `encode_form_query`,
`encode_component`, `read_store(ctx,name).await`, `update_store(ctx,name,closure).await`.
Store is `data_dir/<name>/storage.json` (name ASCII alphanumeric, dash/underscore).
All SDK contexts share a transaction mutex; malformed/non-object JSON is rejected,
failed closure writes nothing, atomic tempfile replacement preserves unknown keys.
Host migration paths are not imported implicitly; port explicitly validates/imports.

`TestContext::new(paths, capabilities)` provides `.context`, `.calls()`, `.events()`,
`.reply(result)` for unit-test capture/reverse responses. Production process tests
must still exercise real pipes; test context is not a replacement for process tests.

Package root: extension.json, bin/<binary>[.exe], resources/**, LICENSE, NOTICE;
`files` manifest object inventories all packaged files except extension.json
(self-hashing is impossible). Catalog pins archive digest and executable digest.
CI must replace local SDK path with parent-approved immutable SDK git revision
before isolated release. Never publish with local path dependency.

See [HOST_INTEGRATION.md](HOST_INTEGRATION.md) for exact reverse argument objects,
renderer error translation, generation ownership, process grace and publication gates.


## Optional launcher contributions (SDK 0.1.1)

Protocol 1 and package schema 3 are unchanged. SDK hello advertises
`features:["launcher.v1"]`. Initialize accepts optional `hostFeatures`; its
response returns the intersection as `features`. `ctx.supports(LAUNCHER_FEATURE)`
checks it. Older SDK 0.1.0 executables and ordinary sessions need no new fields,
hooks, rebuild, or reverse capability. A new SDK connected to an older host keeps
normal sessions and does not publish an index.

`Extension::launcher_activate(ctx, params)` and `launcher_search(ctx, params)`
have default unsupported implementations. A package declaring contributions must
implement the corresponding hooks. Activation is **post ready**, never inside
initialize: only after the host has received `{ready:true}` may it call
`launcher.activate {contributionId}` and accept `host.launcher.publish`. This
separates storage migration from host service readiness.

Generic example (no regular expression evaluation):

```json
{"launcherContributions":[{"id":"items","commandName":"lookup",
 "argumentName":"value","surfaces":["query","argument"],
 "selectors":{"terms":["lookup","catalog"],"digitRun":{"min":2,"max":5}},
 "limit":12,"timeoutMs":1500,"index":true,
 "bootstrapMigrationPaths":["savedItems"]}]}
```

Selectors are case-insensitive substring terms or a maximal ASCII digit run in
an inclusive length range. Host bounds: 8 descriptors/package, 32 terms of up to
128 bytes, digit lengths 1..16, 32 results/query, timeout 50..3000 ms, query UTF-8
length <=1024 bytes. A descriptor targets its own declared command and optional
argument. Host validates these relationships and capability grants on install.

`launcher.search {contributionId,queryId,text,surface,commandName?,argumentName?,limit}`
returns `{rows:[{id,title,subtitle,score,action}]}`. Action is only
`{type:"run-extension-command",commandName:<declared command>,argumentValues:{...}}`;
host derives canonical extension identity, command metadata, and category. Scores
are integers 0..20000. Domain parsing, ranking, OS caches and nonblocking refresh
belong to the extension. Search contexts reject host service RPCs; selection uses
an ordinary command session and freshly performs any required authorization.
Latest searches cancel earlier searches of the same contribution/surface in reader
order. They share the existing bounded handler slots and process task ownership.

With exact capability `launcher.publish`, a negotiated activation/session may send
`host.launcher.publish {contributionId,revision,rows:[...]}`. Index rows add
`tokens:[string]` to the same closed row shape (up to 256 rows, 16 tokens/row,
256 bytes/token, total persisted index <=256 KiB/package). This is a lightweight
label/string-argument index, **not** an OS process inventory. Do not publish process
IDs, identities or listener snapshots. Revisions increase within each process;
host scopes them by process owner, contribution and package fingerprint. Storage
is atomic/private and profile/identity/version/manifest+artifact-digest scoped.
Publish after successful CRUD, serialize snapshot/publication to avoid stale writes,
and propagate publication failures. Rebuild/retry the index after a failed publish;
a persisted domain write cannot be rolled back through a later host RPC.

Host searches index labels without waking every extension. Only selector/index
matches (or current, running contributors' ephemeral matching rows) receive a lazy
query. A missing index may bootstrap once after explicit install/update, or during
bounded activation for an existing declared migration file. No full-extension
startup scan, permanent lease or per-keystroke process enumeration is necessary.
Process startup may run activation again to reconcile the current store. Deep and
empty searches suppress contributors and cancel previous query work.

For async refresh, `ctx.spawn` retains scoped work; emit
`launcher.invalidated` with `{contributionId,queryId,revision}` when ready. This
uses the existing `ctx.emit` envelope, not a session invalidation. Host accepts only
the current query/process and increasing revision, emits a query-specific refresh,
and rejects late results after new query, profile, update or uninstall. Do not
request an enduring background lease merely to retain a cache. Keep refresh work
bounded within the host's idle grace; failed refreshes retain the previous snapshot.

Optional command `launcher` metadata supplies `recommendation` (0..2000), an `id`
restricted to that canonical/replacement command ID, and `inlineArgument`:
`{name,title,prompt,validation?:{kind:"integer",min,max,maxLength,message}}`.
The renderer preserves argument input/Enter/Tab/Escape behavior using this data.
Extension code remains authoritative for validation. Ordinary command execution,
confirmation/dry-run and audit contracts are unchanged.

`ctx.launcher_refresh_context()` is available only inside a launcher search. It
returns a read-only context with process/root task ownership independent of one
query cancellation. Use it for one short bounded cache refresh, publish an
invalidation for the latest query ID, and keep it shorter than host idle grace.
Its `ctx.spawn` is still capped at 64 tasks and stops at EOF/shutdown. It grants no
host service access or enduring background ownership.
