use crate::{CoreResult, MAX_FRAME};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

#[derive(Clone, Debug, Default)]
pub struct ExtensionPaths {
    pub package_dir: PathBuf,
    pub data_dir: PathBuf,
    pub temp_dir: PathBuf,
}
#[derive(Clone)]
pub(crate) struct Cancel(watch::Sender<bool>);
impl Cancel {
    pub(crate) fn new() -> Self {
        Self(watch::channel(false).0)
    }
    pub(crate) fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub(crate) async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        if *rx.borrow() {
            return;
        }
        let _ = rx.wait_for(|v| *v).await;
    }
}
pub(crate) struct Scope {
    pub cancel: Cancel,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}
impl Scope {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel: Cancel::new(),
            tasks: Mutex::new(Vec::new()),
        })
    }
    pub(crate) fn stop(&self) {
        self.cancel.cancel();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.stop();
    }
}

pub const HOST_METHODS: &[&str] = &[
    "host.clipboard.read",
    "host.clipboard.write",
    "host.clipboard.writeImage",
    "host.clipboard.writeFiles",
    "host.clipboard.paste",
    "host.clipboard.pasteText",
    "host.shell.open",
    "host.shell.openWithApp",
    "host.shell.reveal",
    "host.selection.read",
    "host.window.visibility",
    "host.window.suppressBlur",
    "host.dialog.confirm",
    "host.screen.capture",
    "host.apps.list",
    "host.settings.open",
    "host.ai.ask",
    "host.background.retain",
    "host.background.release",
    "host.tray.set",
    "host.tray.remove",
    "host.notifications.show",
    "host.notifications.remove",
    "host.safety.authorize",
    "host.safety.record",
    "host.launcher.publish",
];
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<CoreResult>>>>;
#[derive(Clone)]
pub struct HostClient {
    pub(crate) output: mpsc::Sender<Value>,
    pub(crate) pending: Pending,
    pub(crate) closed: Arc<AtomicBool>,
}
struct Waiter {
    id: String,
    client: HostClient,
    complete: bool,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        self.client.pending.lock().unwrap().remove(&self.id);
        if !self.complete {
            let _ = self.client.output.try_send(
                json!({"type":"cancel","requestId":self.id,"reason":"extension operation dropped"}),
            );
        }
    }
}
impl HostClient {
    pub(crate) fn new(output: mpsc::Sender<Value>) -> Self {
        Self {
            output,
            pending: Arc::default(),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.pending.lock().unwrap().clear();
    }
    pub(crate) fn respond(&self, message: &Value) -> Result<(), String> {
        let id = message["id"].as_str().ok_or("Response id missing")?;
        if !id.starts_with("e:") {
            return Err("Invalid reverse response id".into());
        }
        if message.get("result").is_some() == message.get("error").is_some() {
            return Err("Response must have exactly one result or error".into());
        }
        if let Some(error) = message.get("error") {
            if error["code"].as_str().is_none() || error["message"].as_str().is_none() {
                return Err("Malformed reverse RPC error".into());
            }
        }
        // Unknown IDs can be late replies after cancellation. They never create state.
        if let Some(sender) = self.pending.lock().unwrap().remove(id) {
            let result = if let Some(error) = message.get("error") {
                Err(error["message"]
                    .as_str()
                    .unwrap_or("Host RPC failed")
                    .to_owned())
            } else {
                Ok(message["result"].clone())
            };
            let _ = sender.send(result);
        }
        Ok(())
    }
    pub async fn call(&self, method: &str, params: Value) -> CoreResult {
        if !HOST_METHODS.contains(&method) {
            return Err(format!("Unsupported host method: {method}"));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err("Host connection closed".into());
        }
        let id = format!("e:{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if self.closed.load(Ordering::Acquire) {
                return Err("Host connection closed".into());
            }
            if pending.len() >= 128 {
                return Err("Too many pending host requests".into());
            }
            pending.insert(id.clone(), tx);
        }
        let mut waiter = Waiter {
            id: id.clone(),
            client: self.clone(),
            complete: false,
        };
        self.send(json!({"type":"request","id":id,"method":method,"params":params}))?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(120), rx)
            .await
            .map_err(|_| "Host RPC timed out".to_string())?
            .map_err(|_| "Host connection closed".to_string())?;
        waiter.complete = true;
        result
    }
    pub(crate) fn send(&self, value: Value) -> Result<(), String> {
        if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > MAX_FRAME {
            return Err("Outbound frame too large".into());
        }
        self.output
            .try_send(value)
            .map_err(|_| "Protocol output queue full/closed".into())
    }
}
#[derive(Clone)]
pub struct ExtensionContext {
    pub paths: ExtensionPaths,
    pub platform: String,
    pub migration_paths: HashMap<String, PathBuf>,
    pub granted_capabilities: HashSet<String>,
    pub host_client: HostClient,
    /// Optional protocol features agreed during initialize.
    pub negotiated_features: HashSet<String>,
    pub(crate) launcher_search: bool,
    pub(crate) scope: Arc<Scope>,
    pub(crate) operation_cancel: Cancel,
    pub(crate) session_id: Option<String>,
    pub(crate) revision: Arc<AtomicU64>,
}
impl ExtensionContext {
    pub(crate) fn base(
        paths: ExtensionPaths,
        platform: String,
        migration_paths: HashMap<String, PathBuf>,
        grants: HashSet<String>,
        host_client: HostClient,
    ) -> Self {
        Self {
            paths,
            platform,
            migration_paths,
            granted_capabilities: grants,
            host_client,
            negotiated_features: HashSet::new(),
            launcher_search: false,
            scope: Scope::new(),
            operation_cancel: Cancel::new(),
            session_id: None,
            revision: Arc::default(),
        }
    }
    pub fn supports(&self, feature: &str) -> bool {
        self.negotiated_features.contains(feature)
    }
    pub async fn host(&self, method: &str, params: Value) -> CoreResult {
        if self.launcher_search {
            return Err("Host services are unavailable during launcher search".into());
        }
        if method == "host.launcher.publish" && !self.supports(crate::LAUNCHER_FEATURE) {
            return Err("Launcher feature not negotiated".into());
        }
        let capability = method
            .strip_prefix("host.")
            .ok_or("Expected typed host method")?;
        let group = capability.split('.').next().unwrap();
        if !self.granted_capabilities.contains(method)
            && !self.granted_capabilities.contains(capability)
            && !self.granted_capabilities.contains(group)
        {
            return Err(format!("Host capability not granted: {capability}"));
        }
        tokio::select! { biased; _ = self.cancelled() => Err("Operation cancelled".into()), result = self.host_client.call(method, params) => result }
    }
    pub async fn request(&self, channel: &str, args: Vec<Value>) -> CoreResult {
        let first = args.first().cloned().unwrap_or(Value::Null);
        let second = args.get(1).cloned().unwrap_or(Value::Null);
        let third = args.get(2).cloned().unwrap_or(Value::Null);
        let (method, params) = match channel {
            "shell:open" => ("host.shell.open", json!({"target":first})),
            "clipboard:write" => ("host.clipboard.write", json!({"text":first})),
            "clipboard:read" => ("host.clipboard.read", json!({})),
            "core:platform:selected-text" => ("host.selection.read", json!({})),
            "window:hide" => ("host.window.visibility", json!({"visible":false})),
            "core:platform:visibility" => ("host.window.visibility", json!({"visible":first})),
            "core:platform:suppress-blur" => {
                ("host.window.suppressBlur", json!({"suppress":first}))
            }
            "core:platform:screenshot" => (
                "host.screen.capture",
                if first.is_null() { json!({}) } else { first },
            ),
            "core:platform:clipboard-write" => {
                if first.get("file").is_some() {
                    ("host.clipboard.writeImage", first)
                } else {
                    ("host.clipboard.write", first)
                }
            }
            "core:platform:open-with-app" => ("host.shell.openWithApp", first),
            "core:platform:reveal-file" => ("host.shell.reveal", json!({"path":first})),
            "core:platform:paste-text" => ("host.clipboard.pasteText", json!({"text":first})),
            "core:platform:paste" => (
                "host.clipboard.paste",
                if first.is_null() { json!({}) } else { first },
            ),
            "core:platform:open-settings" => ("host.settings.open", json!({"surface":first})),
            "core:ai:extension-ask" => ("host.ai.ask", first),
            "core:platform:confirm" => ("host.dialog.confirm", first),
            "core:safety:authorize" => (
                "host.safety.authorize",
                json!({"operation":first,"context":second}),
            ),
            "core:safety:record" => (
                "host.safety.record",
                json!({"operation":first,"context":second,"result":third}),
            ),
            _ => return Err(format!("Legacy channel not allowlisted: {channel}")),
        };
        self.host(method, params).await
    }
    pub fn emit(&self, event: &str, data: Value) -> Result<(), String> {
        if self.scope.cancel.is_cancelled() || self.operation_cancel.is_cancelled() {
            return Err("Session/operation cancelled".into());
        }
        let mut message = json!({"type":"event","event":event,"data":data});
        if let Some(id) = &self.session_id {
            message["sessionId"] = json!(id);
        }
        self.host_client.send(message)
    }
    pub fn invalidate(&self) -> Result<(), String> {
        if self.scope.cancel.is_cancelled() || self.operation_cancel.is_cancelled() {
            return Err("Session/operation cancelled".into());
        }
        let id = self
            .session_id
            .as_ref()
            .ok_or("Invalidation requires a session")?;
        self.host_client.send(json!({"type":"event","event":"session.invalidated","sessionId":id,"revision":self.revision.fetch_add(1, Ordering::SeqCst)+1}))
    }
    /// A short process-owned refresh may outlive one launcher query. It remains
    /// read-only, bounded by the root task scope, and is stopped on EOF/shutdown.
    /// The extension must use the CURRENT query ID when invalidating its cache.
    pub fn launcher_refresh_context(&self) -> Result<Self, String> {
        if !self.launcher_search {
            return Err("Refresh context requires launcher search".into());
        }
        let mut ctx = self.clone();
        ctx.operation_cancel = Cancel::new();
        Ok(ctx)
    }
    pub async fn cancelled(&self) {
        tokio::select! { _ = self.scope.cancel.cancelled() => {}, _ = self.operation_cancel.cancelled() => {} }
    }
    pub fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) -> Result<(), String> {
        let mut tasks = self.scope.tasks.lock().unwrap();
        tasks.retain(|t| !t.is_finished());
        if self.scope.cancel.is_cancelled() || tasks.len() >= 64 {
            return Err("Task scope closed/full".into());
        }
        let cancel = self.scope.cancel.clone();
        let operation_cancel = self.operation_cancel.clone();
        tasks.push(tokio::spawn(async move { tokio::select! { biased; _ = cancel.cancelled() => {}, _ = operation_cancel.cancelled() => {}, _ = work => {} } }));
        Ok(())
    }
}
/// Capturing test context. Separate process tests are required for transport verification.
pub struct TestContext {
    pub context: ExtensionContext,
    messages: Mutex<mpsc::Receiver<Value>>,
    calls: Mutex<Vec<Value>>,
    events: Mutex<Vec<Value>>,
    replies: Mutex<std::collections::VecDeque<CoreResult>>,
}
impl TestContext {
    pub fn new(paths: ExtensionPaths, capabilities: &[&str]) -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            context: ExtensionContext::base(
                paths,
                std::env::consts::OS.into(),
                HashMap::new(),
                capabilities.iter().map(|s| s.to_string()).collect(),
                HostClient::new(tx),
            ),
            messages: Mutex::new(rx),
            calls: Mutex::default(),
            events: Mutex::default(),
            replies: Mutex::default(),
        }
    }
    pub fn reply(&self, result: CoreResult) {
        self.replies.lock().unwrap().push_back(result);
    }
    /// Drive capture while awaiting a handler that uses reverse RPC.
    pub async fn run<T>(&self, future: impl Future<Output = T>) -> T {
        tokio::pin!(future);
        loop {
            tokio::select! { result = &mut future => { self.capture(); return result; }, _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => { self.capture(); } }
        }
    }
    fn capture(&self) {
        while let Ok(value) = self.messages.lock().unwrap().try_recv() {
            if value["type"] == "request" {
                self.calls.lock().unwrap().push(value.clone());
                let reply = self
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(Ok(json!(null)));
                if let Some(tx) = self
                    .context
                    .host_client
                    .pending
                    .lock()
                    .unwrap()
                    .remove(value["id"].as_str().unwrap())
                {
                    let _ = tx.send(reply);
                }
            } else {
                self.events.lock().unwrap().push(value);
            }
        }
    }
    pub fn calls(&self) -> Vec<Value> {
        self.capture();
        self.calls.lock().unwrap().clone()
    }
    pub fn events(&self) -> Vec<Value> {
        self.capture();
        self.events.lock().unwrap().clone()
    }
}
impl Drop for TestContext {
    fn drop(&mut self) {
        self.context.scope.stop();
        self.context.host_client.close();
    }
}
