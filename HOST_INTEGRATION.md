# Host integration requirements

Wire contract: [PROTOCOL.md](PROTOCOL.md), revision 1. No app
code is included here. SDK 0.1.0 `SDK_API.md` is the adapter integration surface.

1. Verify archive/catalog identity/version/protocol and target executable hashes
   before spawn. `files` inventories every archive file except extension.json.
   Validate all normal relative paths, assets, symlinks and expanded ZIP bounds.
   The catalog carries full artifact digests; putting a ZIP digest inside its own
   manifest is circular. Source/development archive artifacts may be empty.
2. Launch once per extension/profile, lazily. Read hello first and validate its id,
   version, protocol=1 and supported sdkVersion. Send initialize with absolute
   packageDir/dataDir/tempDir, extensionId, platform, grantedCapabilities,
   migrationPaths and reason. Wait for `{ready:true}` before opening sessions.
   Host must not issue calls on a session before its open response.
3. Host requests use `h:<uuid>`; reverse requests use `e:<uuid>`. Keep the pipe
   reader independent of requests and reverse execution. LF framing is bounded to
   16 MiB on both sides. Errors are `{code:string,message:string}`. Any malformed
   frame ends the process. SDK tolerates unknown/late reverse reply IDs after
   cancellation. Duplicate active host IDs are fatal. Never auto-replay actions.
4. `session.open` params: sessionId, commandName, preferences (object),
   argumentValues (object), searchText/launchContext optional. SDK sends
   `extension:run-command` to adapter. `session.call` carries operation and payload;
   sessionId/preferences/argumentValues in payload are overwritten with snapshots.
   Adapter errors are response.error; renderer adapter must translate to the
   existing `{ok:false,message}` display format. Host annotates canonical
   extensionId/commandName/sessionId on views. SDK returns unchanged/no-view/views.
   Dispose no-view sessions too, to release their bounded SDK slots.
5. Accepted operations are invoke-action, search-text-changed, load-more,
   refresh-session. Dispose uses `session.dispose` and calls dispose-session cleanup.
   Search replacement is fenced at reader dispatch, including lock waiters; refresh
   never waits behind an operation. Manifest concurrency must agree with
   `Extension::concurrency`; Perplexity uses reject-while-busy.
6. Cancellation sends `cancel {requestId,reason}`. Drop any pending renderer request
   on process/session/profile generation changes. On dispose, SDK stops owned tasks
   immediately and invokes bounded cleanup; late generation events must still be
   discarded by host. `session.invalidated` carries sessionId/revision; subscribe
   only for visible views and reject older revisions. Static Perplexity has
   refreshMs=null. Host session IDs must not be reused within a process generation.
7. Host must enforce per-manifest grants, exact method allowlist, argument schemas,
   user consent and host UI ownership. SDK accepts grants `shell.open`,
   `host.shell.open` or broad group `shell`; native binaries are not OS-sandboxed.
   No named-port CRUD, arbitrary channel bridge, AppHandle/config/secrets are exposed.
8. Process references include calls + views + background leases + UI resources.
   `host.background.retain {key,durable,data?}` acknowledges persistent registration
   before domain success. `release {key}` removes it. Restore via initialize reason
   plus background.activate {key,data?}. Tray callbacks use background.event
   {resourceId,actionId,data?}. Resource IDs are extension/generation scoped by host.
   Durable lease records survive host shutdown, and uninstall/disable clears them.
   SDK callbacks receive params unchanged. No feature-specific startup branches.
9. EOF closes pending reverse calls and cancels all session/background scopes.
   Shutdown permits reverse replies while cleanup hooks run, then exits. Per-session
   dispose is bounded to 2s; combined session cleanup bounded to 2s, then shutdown
   hook bounded to 2s. Host must allow this grace and kill tree if exceeded. Control
   slots are reserved so saturation of 120 ordinary handlers cannot block cleanup
   (128 total). POSIX process groups / Windows Job Object and no-console creation
   are host responsibility; host also releases blur/tray/notifications/non-durable
   leases on process death. SDK cannot kill arbitrary unregistered helper threads.
10. Extension publication: replace sibling local SDK dependency with
    immutable git rev, regenerate Cargo.lock and validate isolated extension CI.
    Then create/publish verified target ZIPs, pin generated external artifact
    metadata in trusted host catalog, and run clean-profile real app smoke. Windows
    and live OS integration require actual evidence; CI YAML is not that evidence.

## Reverse RPC argument shapes used by migration helper

| Old channel | Wire method | Params |
|---|---|---|
| shell:open | host.shell.open | `{target: arg0}` |
| clipboard:read/write | host.clipboard.read/write | `{}` / `{text: arg0}` |
| core:platform:selected-text | host.selection.read | `{}` |
| window:hide | host.window.visibility | `{visible:false}` |
| core:platform:visibility | host.window.visibility | `{visible:arg0}` |
| core:platform:suppress-blur | host.window.suppressBlur | `{suppress:arg0}` |
| core:platform:screenshot | host.screen.capture | arg0 object or `{}` |
| core:platform:clipboard-write | host.clipboard.writeImage when file exists, else host.clipboard.write | arg0 object (`file`, or `text`/optional `html`) |
| core:platform:open-with-app | host.shell.openWithApp | arg0 object (`path`,`appPath`) |
| core:platform:reveal-file | host.shell.reveal | `{path:arg0}` |
| core:platform:paste-text | host.clipboard.pasteText | `{text:arg0}` |
| core:platform:paste | host.clipboard.paste | arg0 object or `{}` |
| core:platform:open-settings | host.settings.open | `{surface:arg0}` |
| core:ai:extension-ask | host.ai.ask | arg0 object |
| core:platform:confirm | host.dialog.confirm | arg0 object |
| core:safety:authorize | host.safety.authorize | `{operation:arg0,context:arg1}` |
| core:safety:record | host.safety.record | `{operation:arg0,context:arg1,result:arg2}` |

New handlers should call typed methods directly. Host validates argument objects
before routing. Unsupported old channels return an error rather than falling back.
Perplexity uses only host.shell.open; the other helpers need their port/platform
host integration tests before those ports ship.


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
