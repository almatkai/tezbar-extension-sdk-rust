use crate::{
    context::{Cancel, Scope},
    *,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, Mutex as AsyncMutex},
    task::JoinSet,
};
pub const MAX_FRAME: usize = 16 * 1024 * 1024;
const SEARCH: &str = "extension:search-text-changed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandConcurrency {
    Serialize,
    RejectWhileBusy,
}

pub trait Extension: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn concurrency(&self, _command: &str) -> CommandConcurrency {
        CommandConcurrency::Serialize
    }
    fn commands(&self) -> &'static [&'static str];
    fn handle<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        command: &'a str,
        operation: &'a str,
        payload: &'a Value,
        state: &'a mut Value,
    ) -> BoxFuture<'a, CoreResult>;
    fn initialize<'a>(&'a self, _ctx: &'a ExtensionContext) -> BoxFuture<'a, CoreResult> {
        Box::pin(async { Ok(json!({"ready":true})) })
    }
    fn background_activate<'a>(
        &'a self,
        _ctx: &'a ExtensionContext,
        _params: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async { Err("Background activation unsupported".into()) })
    }
    fn background_event<'a>(
        &'a self,
        _ctx: &'a ExtensionContext,
        _params: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async { Err("Background event unsupported".into()) })
    }
    /// Called only after initialize has been acknowledged by the host. Publish
    /// lightweight persisted labels here, never during initialize.
    fn launcher_activate<'a>(
        &'a self,
        _ctx: &'a ExtensionContext,
        _params: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async { Err("Launcher activation unsupported".into()) })
    }
    /// Search is bounded/cancellable and independent of command session state.
    /// Domain caches and ranking belong to the extension.
    fn launcher_search<'a>(
        &'a self,
        _ctx: &'a ExtensionContext,
        _params: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async { Err("Launcher search unsupported".into()) })
    }
    fn shutdown<'a>(&'a self, _ctx: &'a ExtensionContext) -> BoxFuture<'a, CoreResult> {
        Box::pin(async { Ok(json!({})) })
    }
}
struct Session {
    command: String,
    preferences: Value,
    arguments: Value,
    state: AsyncMutex<Value>,
    scope: Arc<Scope>,
    revision: Arc<AtomicU64>,
    search: Mutex<(u64, Cancel)>,
}
type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;
struct Registration {
    id: String,
    active: Arc<Mutex<HashMap<String, Cancel>>>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.active.lock().unwrap().remove(&self.id);
    }
}
struct RunnerCleanup {
    host: Option<HostClient>,
    writer: Option<tokio::task::AbortHandle>,
    sessions: Sessions,
    active: Arc<Mutex<HashMap<String, Cancel>>>,
    root: Arc<Mutex<Option<Arc<Scope>>>>,
}
impl Drop for RunnerCleanup {
    fn drop(&mut self) {
        if let Some(host) = &self.host {
            host.close();
        }
        if let Some(writer) = &self.writer {
            writer.abort();
        }
        for token in self.active.lock().unwrap().values() {
            token.cancel();
        }
        for session in self.sessions.lock().unwrap().values() {
            session.scope.stop();
        }
        if let Some(scope) = self.root.lock().unwrap().as_ref() {
            scope.stop();
        }
    }
}
fn text<'a>(v: &'a Value, field: &str) -> Result<&'a str, String> {
    v[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing/invalid {field}"))
}
fn response(id: &str, result: CoreResult) -> Value {
    match result {
        Ok(result) => json!({"type":"response","id":id,"result":result}),
        Err(message) => {
            json!({"type":"response","id":id,"error":{"code":"extension_error","message":message}})
        }
    }
}
async fn frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    bytes: &mut Vec<u8>,
) -> Result<Option<Value>, String> {
    loop {
        let chunk = reader.fill_buf().await.map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("Truncated frame at EOF".into())
            };
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let count = end.map(|i| i + 1).unwrap_or(chunk.len());
        let data_len = if end.is_some() { count - 1 } else { count };
        if bytes.len() + data_len > MAX_FRAME {
            return Err("Frame exceeds 16 MiB".into());
        }
        bytes.extend_from_slice(&chunk[..data_len]);
        reader.consume(count);
        if end.is_some() {
            if bytes.contains(&b'\r') {
                return Err("Frames must use LF, not CR".into());
            }
            let v: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("Malformed JSON frame: {e}"))?;
            if !v.is_object() {
                return Err("Frame must be object".into());
            }
            bytes.clear();
            return Ok(Some(v));
        }
    }
}
fn initialize(
    params: &Value,
    extension: &dyn Extension,
    host: HostClient,
) -> Result<ExtensionContext, String> {
    if params["protocol"] != 1 || params["extensionId"] != extension.id() {
        return Err("Initialize identity/protocol mismatch".into());
    }
    let paths = ExtensionPaths {
        package_dir: text(params, "packageDir")?.into(),
        data_dir: text(params, "dataDir")?.into(),
        temp_dir: text(params, "tempDir")?.into(),
    };
    if ![&paths.package_dir, &paths.data_dir, &paths.temp_dir]
        .iter()
        .all(|p| p.is_absolute())
    {
        return Err("Scoped paths must be absolute".into());
    }
    let grants = params["grantedCapabilities"]
        .as_array()
        .ok_or("grantedCapabilities must be an array")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or("Capability must be string".to_string())
        })
        .collect::<Result<_, _>>()?;
    let mut migrations = HashMap::new();
    if let Some(v) = params.get("migrationPaths") {
        for (key, value) in v.as_object().ok_or("migrationPaths must be object")? {
            let path: std::path::PathBuf = value
                .as_str()
                .ok_or("Migration path must be string")?
                .into();
            if !path.is_absolute() {
                return Err("Migration path must be absolute".into());
            }
            migrations.insert(key.clone(), path);
        }
    }
    let mut ctx = ExtensionContext::base(
        paths,
        text(params, "platform")?.into(),
        migrations,
        grants,
        host,
    );
    if params["hostFeatures"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == LAUNCHER_FEATURE))
    {
        ctx.negotiated_features.insert(LAUNCHER_FEATURE.into());
    }
    Ok(ctx)
}
fn session_context(
    base: &ExtensionContext,
    id: &str,
    session: &Session,
    cancel: Cancel,
) -> ExtensionContext {
    let mut ctx = base.clone();
    ctx.scope = session.scope.clone();
    ctx.session_id = Some(id.into());
    ctx.revision = session.revision.clone();
    ctx.operation_cancel = cancel;
    ctx
}
struct StopScope(Arc<Scope>);
impl Drop for StopScope {
    fn drop(&mut self) {
        self.0.stop();
    }
}
async fn dispose(
    extension: &dyn Extension,
    base: &ExtensionContext,
    id: &str,
    session: &Session,
) -> CoreResult {
    session.scope.stop();
    let cleanup_scope = Scope::new();
    let _scope_guard = StopScope(cleanup_scope.clone());
    let mut ctx = session_context(base, id, session, Cancel::new());
    ctx.scope = cleanup_scope.clone();
    let payload = json!({"sessionId":id,"preferences":session.preferences,"argumentValues":session.arguments});
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let mut state = session.state.lock().await;
        extension
            .handle(
                &ctx,
                &session.command,
                "extension:dispose-session",
                &payload,
                &mut state,
            )
            .await
    })
    .await
    .map_err(|_| "Dispose cleanup timed out".to_string());
    cleanup_scope.stop();
    result?
}
async fn shutdown_hook(extension: &dyn Extension, base: &ExtensionContext) -> CoreResult {
    base.scope.stop();
    let mut ctx = base.clone();
    ctx.scope = Scope::new();
    ctx.operation_cancel = Cancel::new();
    let _scope_guard = StopScope(ctx.scope.clone());
    tokio::time::timeout(Duration::from_secs(2), extension.shutdown(&ctx))
        .await
        .map_err(|_| "Shutdown cleanup timed out".to_string())?
}
struct CallControl {
    cancel: Cancel,
    search: Option<(u64, Cancel)>,
}
async fn call(
    extension: &dyn Extension,
    base: &ExtensionContext,
    id: &str,
    session: Arc<Session>,
    operation: &str,
    mut payload: Value,
    control: CallControl,
) -> CoreResult {
    let CallControl { cancel, search } = control;
    if !payload.is_object() {
        return Err("Call payload must be object".into());
    }
    payload["sessionId"] = json!(id);
    payload["preferences"] = session.preferences.clone();
    payload["argumentValues"] = session.arguments.clone();
    let ctx = session_context(base, id, &session, cancel.clone());
    let execute = async {
        let mut state = if operation == "extension:refresh-session"
            || (operation == "extension:invoke-action"
                && extension.concurrency(&session.command) == CommandConcurrency::RejectWhileBusy)
        {
            match session.state.try_lock() {
                Ok(state) => state,
                Err(_) => {
                    return if operation == "extension:refresh-session" {
                        Ok(unchanged())
                    } else {
                        Err("Command is busy".into())
                    }
                }
            }
        } else {
            session.state.lock().await
        };
        let mut staged = state.clone();
        let result = extension
            .handle(&ctx, &session.command, operation, &payload, &mut staged)
            .await?;
        if session.scope.cancel.is_cancelled() || cancel.is_cancelled() {
            return Err("Operation cancelled".into());
        }
        if let Some((epoch, _)) = &search {
            let fence = session.search.lock().unwrap();
            if fence.0 != *epoch {
                return Err("Search superseded".into());
            }
            *state = staged;
        } else {
            *state = staged;
        }
        Ok(result)
    };
    tokio::select! { biased;
        _ = session.scope.cancel.cancelled() => Err("Session disposed".into()),
        _ = cancel.cancelled() => Err("Operation cancelled".into()),
        _ = async { match &search { Some((_,token)) => token.cancelled().await, None => std::future::pending().await } } => Err("Search superseded".into()),
        result = execute => result,
    }
}

pub async fn serve<E: Extension>(extension: E) -> Result<(), String> {
    if std::env::args().any(|a| a == "--protocol-info" || a == "--self-test") {
        println!(
            "{}",
            json!({"protocol":1,"id":extension.id(),"version":extension.version(),"sdkVersion":env!("CARGO_PKG_VERSION"),"commands":extension.commands()})
        );
        return Ok(());
    }
    serve_io(
        extension,
        crate::stdio::input().map_err(|e| e.to_string())?,
        tokio::io::stdout(),
    )
    .await
}
pub async fn serve_io<E, R, W>(extension: E, input: R, output: W) -> Result<(), String>
where
    E: Extension,
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let extension = Arc::new(extension);
    let (tx, mut rx) = mpsc::channel::<Value>(256);
    let host = HostClient::new(tx.clone());
    let mut writer = tokio::spawn(async move {
        let mut output = output;
        while let Some(v) = rx.recv().await {
            let mut bytes = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
            if bytes.len() > MAX_FRAME {
                return Err("Outbound frame exceeds 16 MiB".into());
            }
            bytes.push(b'\n');
            output.write_all(&bytes).await.map_err(|e| e.to_string())?;
            output.flush().await.map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(())
    });
    host.send(json!({"type":"hello","protocol":1,"id":extension.id(),"version":extension.version(),"sdkVersion":env!("CARGO_PKG_VERSION"),"features":[LAUNCHER_FEATURE]}))?;
    let mut input = BufReader::new(input);
    // Retain partial frames when select is interrupted by handler/writer completion.
    let mut inbound = Vec::new();
    let mut tasks = JoinSet::new();
    let active: Arc<Mutex<HashMap<String, Cancel>>> = Arc::default();
    let sessions: Sessions = Arc::default();
    let launcher_queries: Arc<Mutex<HashMap<String, Cancel>>> = Arc::default();
    let ready = Arc::new(AtomicBool::new(false));
    let mut base: Option<ExtensionContext> = None;
    let (done_tx, mut done_rx) = mpsc::channel(1);
    let mut shutting_down = false;
    let mut shutdown_done = false;
    let mut _cleanup = RunnerCleanup {
        host: Some(host.clone()),
        writer: Some(writer.abort_handle()),
        sessions: sessions.clone(),
        active: active.clone(),
        root: Arc::new(Mutex::new(None)),
    };
    let outcome = 'runner: loop {
        while let Some(result) = tasks.try_join_next() {
            if let Err(e) = result {
                break 'runner Err(format!("Handler task failed: {e}"));
            }
        }
        let incoming = tokio::select! {
            result = frame(&mut input, &mut inbound) => result,
            _ = done_rx.recv() => { shutdown_done = true; break Ok(()); },
            result = &mut writer => { break Err(format!("Protocol writer closed: {result:?}")); },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => { if let Err(e) = result { break Err(format!("Handler task failed: {e}")); } continue; }
        };
        let message = match incoming {
            Ok(Some(v)) => v,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        match message["type"].as_str() {
            Some("response") => {
                if let Err(e) = host.respond(&message) {
                    break Err(e);
                }
            }
            Some("cancel") => {
                let Some(id) = message["requestId"].as_str() else {
                    break Err("Cancel requestId missing".into());
                };
                if let Some(token) = active.lock().unwrap().get(id) {
                    token.cancel();
                }
            }
            Some("event") => { /* Host events currently have no defined protocol-1 subscription. */
            }
            Some("request") => {
                let id = match text(&message, "id") {
                    Ok(id)
                        if id
                            .strip_prefix("h:")
                            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()) =>
                    {
                        id.to_owned()
                    }
                    _ => break Err("Invalid host request id".into()),
                };
                if active.lock().unwrap().contains_key(&id) {
                    break Err("Duplicate active request id".into());
                }
                let method = match text(&message, "method") {
                    Ok(v) => v.to_owned(),
                    Err(e) => {
                        if let Err(e) = host.send(response(&id, Err(e))) {
                            break Err(e);
                        }
                        continue;
                    }
                };
                // Reserve eight bounded control slots so a saturated action queue
                // cannot prevent disposal/shutdown from cancelling that queue.
                let limit = if matches!(method.as_str(), "session.dispose" | "shutdown") {
                    128
                } else {
                    120
                };
                if active.lock().unwrap().len() >= limit || shutting_down {
                    if let Err(e) =
                        host.send(response(&id, Err("Runner busy/shutting down".into())))
                    {
                        break Err(e);
                    }
                    continue;
                }
                let params = message.get("params").cloned().unwrap_or(json!({}));
                if !params.is_object() {
                    if let Err(e) = host.send(response(&id, Err("Params must be object".into()))) {
                        break Err(e);
                    }
                    continue;
                }
                if method == "initialize" {
                    if base.is_some() {
                        if let Err(e) = host.send(response(&id, Err("Already initialized".into())))
                        {
                            break Err(e);
                        }
                        continue;
                    }
                    match initialize(&params, extension.as_ref(), host.clone()) {
                        Ok(ctx) => {
                            *_cleanup.root.lock().unwrap() = Some(ctx.scope.clone());
                            base = Some(ctx);
                        }
                        Err(e) => {
                            if let Err(e) = host.send(response(&id, Err(e))) {
                                break Err(e);
                            }
                            continue;
                        }
                    }
                } else if !ready.load(Ordering::Acquire) {
                    if let Err(e) = host.send(response(&id, Err("Not initialized".into()))) {
                        break Err(e);
                    }
                    continue;
                }
                let mut ctx = base.as_ref().unwrap().clone();
                let cancel = Cancel::new();
                ctx.operation_cancel = cancel.clone();
                ctx.launcher_search = method == "launcher.search";
                // Dispatch search/dispose in reader order, before worker scheduling or state locks.
                let session_id = params["sessionId"].as_str().unwrap_or("").to_owned();
                let mut selected = None;
                let mut search = None;
                let preparation: Result<(), String> = (|| {
                    match method.as_str() {
                        "session.open" => {
                            if session_id.is_empty() {
                                return Err("Missing sessionId".into());
                            }
                            let command = text(&params, "commandName")?;
                            if !extension.commands().contains(&command) {
                                return Err("Unknown command".into());
                            }
                            for field in ["preferences", "argumentValues"] {
                                if params.get(field).is_some_and(|v| !v.is_object()) {
                                    return Err(format!("{field} must be object"));
                                }
                            }
                            let mut map = sessions.lock().unwrap();
                            if map.len() >= 128 || map.contains_key(&session_id) {
                                return Err("Session exists/session limit reached".into());
                            }
                            let session = Arc::new(Session {
                                command: command.into(),
                                preferences: params
                                    .get("preferences")
                                    .cloned()
                                    .unwrap_or(json!({})),
                                arguments: params
                                    .get("argumentValues")
                                    .cloned()
                                    .unwrap_or(json!({})),
                                state: AsyncMutex::new(json!({})),
                                scope: Scope::new(),
                                revision: Arc::default(),
                                search: Mutex::new((0, Cancel::new())),
                            });
                            map.insert(session_id.clone(), session.clone());
                            selected = Some(session);
                        }
                        "session.call" => {
                            let session = sessions
                                .lock()
                                .unwrap()
                                .get(&session_id)
                                .cloned()
                                .ok_or("Unknown session")?;
                            let operation = text(&params, "operation")?;
                            if ![
                                "extension:invoke-action",
                                SEARCH,
                                "extension:load-more",
                                "extension:refresh-session",
                            ]
                            .contains(&operation)
                            {
                                return Err("Unsupported session operation".into());
                            }
                            if operation == SEARCH {
                                let mut fence = session.search.lock().unwrap();
                                fence.1.cancel();
                                fence.0 += 1;
                                fence.1 = cancel.clone();
                                search = Some((fence.0, fence.1.clone()));
                            }
                            selected = Some(session);
                        }
                        "session.dispose" => {
                            let session = sessions
                                .lock()
                                .unwrap()
                                .remove(&session_id)
                                .ok_or("Unknown session")?;
                            session.scope.stop();
                            selected = Some(session);
                        }
                        "launcher.activate" | "launcher.search" => {
                            if !ctx.supports(LAUNCHER_FEATURE) {
                                return Err("Launcher feature not negotiated".into());
                            }
                            let contribution = text(&params, "contributionId")?;
                            if contribution.len() > 128 {
                                return Err("Invalid contribution ID".into());
                            }
                            if method == "launcher.search" {
                                let query = text(&params, "queryId")?;
                                if query.len() > 128
                                    || params["text"].as_str().is_none_or(|s| s.len() > 1024)
                                    || !matches!(
                                        params["surface"].as_str(),
                                        Some("query" | "argument")
                                    )
                                    || params["limit"]
                                        .as_u64()
                                        .is_none_or(|n| !(1..=32).contains(&n))
                                {
                                    return Err("Invalid launcher search".into());
                                }
                                if params["surface"] == "argument"
                                    && !extension
                                        .commands()
                                        .contains(&text(&params, "commandName")?)
                                {
                                    return Err("Unknown argument command".into());
                                }
                                let key = format!(
                                    "{}:{}",
                                    contribution,
                                    params["surface"].as_str().unwrap()
                                );
                                let mut queries = launcher_queries.lock().unwrap();
                                if !queries.contains_key(&key) && queries.len() >= 32 {
                                    return Err("Too many launcher contributions".into());
                                }
                                if let Some(previous) = queries.insert(key, cancel.clone()) {
                                    previous.cancel();
                                }
                            }
                        }
                        "initialize" | "background.activate" | "background.event" => {}
                        "shutdown" => {
                            shutting_down = true;
                        }
                        _ => return Err(format!("Unsupported host method: {method}")),
                    }
                    Ok(())
                })();
                if let Err(e) = preparation {
                    if let Err(e) = host.send(response(&id, Err(e))) {
                        break Err(e);
                    }
                    continue;
                }
                active.lock().unwrap().insert(id.clone(), cancel.clone());
                let registration = Registration {
                    id: id.clone(),
                    active: active.clone(),
                };
                let extension = extension.clone();
                let sessions = sessions.clone();
                let ready = ready.clone();
                let output = tx.clone();
                let done = done_tx.clone();
                tasks.spawn(async move {
                    let _registration = registration;
                    let work = async {
                        match method.as_str() {
                            "initialize" => { extension.initialize(&ctx).await?; ready.store(true,Ordering::Release); Ok(json!({"ready":true,"features":ctx.negotiated_features})) },
                            "session.open" | "session.call" => {
                                let session = selected.unwrap();
                                let operation = if method == "session.open" { "extension:run-command" } else { params["operation"].as_str().unwrap() };
                                let payload = if method == "session.open" { params.clone() } else { params.get("payload").cloned().unwrap_or(json!({})) };
                                let result = call(extension.as_ref(),&ctx,&session_id,session.clone(),operation,payload,CallControl { cancel: cancel.clone(), search }).await;
                                if method == "session.open" && result.is_err() {
                                    let mut map = sessions.lock().unwrap();
                                    if map.get(&session_id).is_some_and(|s| Arc::ptr_eq(s,&session)) { map.remove(&session_id); }
                                    session.scope.stop();
                                }
                                result
                            },
                            "session.dispose" => dispose(extension.as_ref(),&ctx,&session_id,&selected.unwrap()).await,
                            "launcher.activate" => extension.launcher_activate(&ctx,&params).await,
                            "launcher.search" => extension.launcher_search(&ctx,&params).await,
                            "background.activate" => extension.background_activate(&ctx,&params).await,
                            "background.event" => extension.background_event(&ctx,&params).await,
                            "shutdown" => {
                                let all: Vec<_> = sessions.lock().unwrap().drain().collect();
                                for (_,s) in &all { s.scope.stop(); }
                                let _ = tokio::time::timeout(Duration::from_secs(2), async { for (id,s) in all { let _ = dispose(extension.as_ref(),&ctx,&id,&s).await; } }).await;
                                shutdown_hook(extension.as_ref(),&ctx).await
                            },
                            _ => unreachable!(),
                        }
                    };
                    // Session paths own cancellation so failed opens always remove their registration.
                    let result = if method.starts_with("session.") || method == "shutdown" { work.await } else { tokio::select! { biased; _ = cancel.cancelled() => Err("Operation cancelled".into()), result = work => result } };
                    let mut reply = response(&id,result);
                    if serde_json::to_vec(&reply).map_or(true, |bytes| bytes.len() > MAX_FRAME) {
                        reply = response(&id,Err("Outbound frame exceeds 16 MiB".into()));
                    }
                    let _ = output.send(reply).await;
                    if method == "shutdown" { let _ = done.send(()).await; }
                });
            }
            _ => break Err("Unknown frame type".into()),
        }
    };
    host.close();
    for token in active.lock().unwrap().values() {
        token.cancel();
    }
    let all: Vec<_> = sessions.lock().unwrap().drain().collect();
    for (_, session) in &all {
        session.scope.stop();
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Some(ctx) = base.as_ref() {
        ctx.scope.stop();
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            for (id, s) in all {
                let _ = dispose(extension.as_ref(), ctx, &id, &s).await;
            }
        })
        .await;
        if !shutdown_done {
            let _ = shutdown_hook(extension.as_ref(), ctx).await;
        }
        ctx.scope.stop();
    }
    drop(base);
    drop(host);
    drop(tx);
    drop(done_tx);
    _cleanup.host.take();
    _cleanup.writer.take();
    if !writer.is_finished()
        && tokio::time::timeout(Duration::from_secs(2), &mut writer)
            .await
            .is_err()
    {
        writer.abort();
        let _ = writer.await;
    }
    outcome
}
