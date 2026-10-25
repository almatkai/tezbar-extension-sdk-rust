use crate::{CoreResult, ExtensionContext};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Mutex};
// One shared lane includes separately constructed contexts. No lost update/retry of FnOnce.
static STORE_LOCK: Mutex<()> = Mutex::new(());
fn path(ctx: &ExtensionContext, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("Invalid storage name".into());
    }
    Ok(ctx.paths.data_dir.join(name).join("storage.json"))
}
fn read(path: &std::path::Path) -> CoreResult {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e.to_string()),
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if !value.is_object() {
                return Err("Storage must be a JSON object".into());
            }
            Ok(value)
        }
    }
}
pub async fn read_store(ctx: &ExtensionContext, name: &str) -> CoreResult {
    let path = path(ctx, name)?;
    tokio::task::spawn_blocking(move || {
        let _guard = STORE_LOCK.lock().map_err(|e| e.to_string())?;
        read(&path)
    })
    .await
    .map_err(|e| e.to_string())?
}
pub async fn update_store<F>(ctx: &ExtensionContext, name: &str, change: F) -> CoreResult
where
    F: FnOnce(&mut Value) -> Result<(), String> + Send + 'static,
{
    let path = path(ctx, name)?;
    tokio::task::spawn_blocking(move || {
        let _guard = STORE_LOCK.lock().map_err(|e| e.to_string())?;
        let mut value = read(&path)?;
        change(&mut value)?;
        if !value.is_object() {
            return Err("Storage must remain a JSON object".into());
        }
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temp.write_all(&serde_json::to_vec(&value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(value)
    })
    .await
    .map_err(|e| e.to_string())?
}
