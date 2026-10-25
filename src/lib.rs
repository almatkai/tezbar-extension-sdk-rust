//! Independent process SDK. See SDK_API.md for ownership and wire guarantees.
mod context;
mod runner;
mod stdio;
mod storage;
pub use context::*;
pub use runner::{serve, serve_io, CommandConcurrency, Extension, MAX_FRAME};
use serde_json::{json, Value};
pub use storage::{read_store, update_store};
pub type CoreResult = Result<Value, String>;
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
pub fn view(title: &str, root: Value, actions: Vec<Value>) -> Value {
    json!({"ok":true,"mode":"view","title":title,"root":root,"actions":actions})
}
pub fn no_view(message: &str) -> Value {
    json!({"ok":true,"mode":"no-view","message":message})
}
pub fn unchanged() -> Value {
    json!({"ok":true,"mode":"unchanged"})
}
pub async fn open(ctx: &ExtensionContext, target: &str) -> CoreResult {
    ctx.host("host.shell.open", json!({"target":target})).await
}
pub async fn copy(ctx: &ExtensionContext, text: &str) -> CoreResult {
    ctx.host("host.clipboard.write", json!({"text":text})).await
}
fn encode(text: &str, form: bool) -> String {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"*-._".contains(&b) || (!form && b"!~'()".contains(&b)) {
            out.push(b as char);
        } else if form && b == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    out
}
pub fn encode_form_query(text: &str) -> String {
    encode(text, true)
}
pub fn encode_component(text: &str) -> String {
    encode(text, false)
}

/// Additive protocol-1 launcher hooks; never required for ordinary sessions.
pub const LAUNCHER_FEATURE: &str = "launcher.v1";
