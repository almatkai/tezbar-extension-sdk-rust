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

/// Additive protocol-1 capture contract; old hosts retain active-only capture.
pub const CAPTURE_FEATURE: &str = "screen.capture.targets.v1";
#[derive(Clone, Copy, Debug)]
pub enum CaptureTarget { Active, All, InteractiveRegion }
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct CapturedImage { pub display_id: String, pub mime_type: String, pub data: String, pub width: u32, pub height: u32 }
#[derive(Debug, serde::Deserialize)]
#[serde(tag="status", rename_all="lowercase", deny_unknown_fields)]
pub enum CaptureReply { Captured { images: Vec<CapturedImage> }, Cancelled { images: Vec<CapturedImage> } }
impl CaptureReply {
    pub fn parse(value: Value) -> Result<Self,String> {
        let reply: Self = serde_json::from_value(value).map_err(|e|e.to_string())?;
        match &reply {
            Self::Cancelled { images } if !images.is_empty() => return Err("Cancelled capture contains pixels".into()),
            Self::Captured { images } => {
                if images.is_empty() || images.len()>16 { return Err("Invalid capture count".into()); }
                let mut total=0; let mut ids=std::collections::HashSet::new();
                for i in images {
                    total+=i.data.len();
                    if !ids.insert(&i.display_id) || i.mime_type!="image/png" || i.width==0 || i.height==0 || i.width>4096 || i.height>4096 || i.data.len()>4*1024*1024 || total>12*1024*1024 { return Err("Invalid capture image bounds".into()); }
                }
            }, _ => {}
        }
        Ok(reply)
    }
}
#[cfg(test)]
mod capture_tests {
    use super::*;
    #[test] fn validates_cancel_and_bounds() {
        assert!(CaptureReply::parse(json!({"status":"cancelled","images":[]})).is_ok());
        assert!(CaptureReply::parse(json!({"status":"captured","images":[]})).is_err());
        let image=json!({"displayId":"0","mimeType":"image/png","data":"a","width":32,"height":32});
        assert!(CaptureReply::parse(json!({"status":"cancelled","images":[image.clone()]})).is_err());
        assert!(CaptureReply::parse(json!({"status":"captured","images":[image.clone(),image]})).is_err());
    }
}
