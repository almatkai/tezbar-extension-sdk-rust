# External Rust extension protocol, revision 1

Standalone SDK and extension executables build independently of Tezbar and Tauri. This document defines protocol 1; incompatible wire changes require a new protocol version. See [SDK_API.md](SDK_API.md) for the Rust interface and [HOST_INTEGRATION.md](HOST_INTEGRATION.md) for host requirements.

## Manifest and transport
- Schema 3; runtime rust; execution process; protocolVersion 1. Canonical com.tezbar.native.<slug>, existing command IDs and declared platform gaps preserved.
- package manifest extension.json: id,slug,name,version,repository,commands,preferences,replaces,capabilities,artifacts. Source manifest may have empty artifacts only during development; published host catalog must pin real target artifacts. Per-command optional timeoutMs (host cap 300000), concurrency (serialize/reject-while-busy), refreshMs (null/static or integer >=500).
- Targets aarch64-apple-darwin and x86_64-pc-windows-msvc. One ZIP format for both: root extension.json, bin/<name>[.exe], resources/**, LICENSE/NOTICE. Artifact fields url,sha256,executable,executableSha256; archive manifest file inventory path->sha256 validates packaged assets. Archive/executable identity checked before launch. Local artifact installation supported for development/private repositories; no user-machine Cargo build.
- UTF-8 JSON-lines, bounded 16MiB frames, LF only, stdin/stdout protocol only, stderr logs. Same envelope in both directions: {type:request,id:h:<uuid>|e:<uuid>,method,params}; response {type:response,id,result} OR {type:response,id,error:{code,message}}; cancel {type:cancel,requestId,reason}; event {type:event,event,sessionId?,revision?,data?}.
- First child frame {type:hello,protocol:1,id:<extensionId>,version:<version>,sdkVersion:<version>}. Host validates selected identity/version/protocol before requests. initialize params {protocol:1,extensionId,platform,packageDir,dataDir,tempDir,grantedCapabilities,reason,migrationPaths:{...}} -> {ready:true}. dataDir/tempDir are extension private; packageDir is validated package root. Never expose all app configuration or credentials. Migration paths are explicitly allowlisted host-controlled paths from trusted manifest, not child-selected arbitrary host reads.
- Host methods: initialize; session.open {sessionId,commandName,preferences,argumentValues,searchText?,launchContext?}; session.call {sessionId,operation,payload}; session.dispose {sessionId}; background.activate {key,data?}; background.event {resourceId,actionId,data?}; shutdown.
- session.call operation uses existing extension:* channel names; payload contains current renderer request fields. SDK augments with canonical sessionId, preferences, argumentValues. Response uses existing view/no-view/unchanged format. Host annotates canonical identity and validates session ownership. event session.invalidated is scoped and revisioned.
- SDK API: ExtensionContext (paths.package_dir/data_dir/temp_dir, negotiated capabilities and HostClient), CoreResult=Result<Value,String>, async host(method,params), emit/invalidate methods. Independent serve runner with command list and handle(ctx,command,operation,payload,&mut state) adapter pattern, lifecycle init/background/shutdown hooks. No fake native_core module or app dependency. SDK can expose a carefully allowlisted legacy channel mapping helper for mechanical extraction, but host wire is typed host.* methods.
- RPC reader must remain independent of handler execution/reverse RPC; bounded queues/pending. Session state serialization, latest-search-wins cancellation and transactional search state commits. Refresh try-lock returns unchanged. On EOF/cancel/shutdown dispose session tasks and helpers; no detached tasks beyond explicit background ownership.

## Generic capabilities and lifecycle
- Typed capabilities/methods: host.clipboard.read/write/writeImage/writeFiles/paste/pasteText; host.shell.open/openWithApp/reveal; host.selection.read; host.window.visibility/suppressBlur; host.dialog.confirm; host.screen.capture; host.apps.list; host.settings.open; host.ai.ask. Host checks exact allowlist+manifest grants; no arbitrary core channel proxy.
- Storage in SDK extension-private data dir, atomic JSON replacement with corruption rejection and one shared transaction mutex per store; host supplies scoped paths and runs one process per extension/profile. Preserve existing store contents via explicit migration paths. Host storage RPC may be added for generic CAS, but no app-global config access.
- host.background.retain {key,durable,data?} -> acknowledgement before domain action is acknowledged; host.background.release {key}. Host persists durable activations per installed extension and resumes via background.activate. No extension-slug branch in startup. One-time startupProbe manifest policy for migration. Refs = in-flight+views+leases+UI resources; stop after15sidle whenzero. Never start child for catalog/listing or unrelated searches. Timers durable, keepawake non-durable, tray scopedretain.
- host.tray.set {resourceId,title,tooltip?,icon?,items:[{id,title,enabled?,checked?}]} and host.tray.remove {resourceId}; events background.event resourceId/actionId; host derives OS IDs and releases on process death. No Tauri types in extension.
- host.notifications.show {id,title,body,action?} and host.notifications.remove {id}; host derives OS IDs; no timer-specific protocol. Generic resource cleanup on process shutdown.
- host.safety.authorize {operation,context} / host.safety.record {operation,context,result} preserve existing host confirmation/dry-run/audit behavior for dangerous OS actions; Port Manager owns named-port data and PID identity/OS signaling implementation, not core named-port CRUD.
- Process shutdown kills child tree; POSIX groups and Windows Job Object/no-console if available. Bounded deadlines, no auto-replay side-effect actions. Profile/extension generation fencing; uninstall/update affects only target. Legacy JS compatibility remains for out-of-scope packages and uncovered commands; canonical invalid packages never fall into JS.


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
