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
