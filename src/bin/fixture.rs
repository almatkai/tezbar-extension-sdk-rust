use serde_json::{json, Value};
use tezbar_extension_sdk::*;
struct Fixture;
impl Extension for Fixture {
    fn id(&self) -> &'static str {
        "com.tezbar.native.fixture"
    }
    fn version(&self) -> &'static str {
        "1.0.0"
    }
    fn commands(&self) -> &'static [&'static str] {
        &["fixture"]
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        _command: &'a str,
        op: &'a str,
        payload: &'a Value,
        state: &'a mut Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            match op {
                "extension:run-command" => Ok(view("Қазақ 😀", json!({"type":"List"}), vec![])),
                "extension:dispose-session" => Ok(unchanged()),
                "extension:search-text-changed" => {
                    state["query"] = payload["searchText"].clone();
                    if payload["searchText"] == "slow" {
                        ctx.host("host.shell.open", json!({"target":"slow"}))
                            .await?;
                    }
                    Ok(state.clone())
                }
                "extension:invoke-action" => match payload["actionId"].as_str().unwrap_or("") {
                    "reverse" => {
                        ctx.host("host.shell.open", json!({"target":payload["value"]}))
                            .await
                    }
                    "delay" => {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        Ok(unchanged())
                    }
                    "large" => Ok(json!("x".repeat(MAX_FRAME))),
                    "scope-limit" => {
                        let mut accepted = 0;
                        for _ in 0..65 {
                            if ctx.spawn(std::future::pending()).is_ok() {
                                accepted += 1;
                            }
                        }
                        Ok(json!({"accepted":accepted}))
                    }
                    "spawn" => {
                        let child = ctx.clone();
                        ctx.spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            let _ = child.emit("late", json!({}));
                        })
                        .unwrap();
                        Ok(unchanged())
                    }
                    "invalidate" => {
                        ctx.invalidate()?;
                        Ok(unchanged())
                    }
                    _ => Ok(state.clone()),
                },
                _ => Ok(state.clone()),
            }
        })
    }
    fn launcher_activate<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        _p: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            ctx.host("host.launcher.publish", json!({"contributionId":"items","revision":1,"rows":[{
                "id":"fixture:label","title":"雪 café 😀","subtitle":"Saved item","score":430,"tokens":["雪 café 😀"],
                "action":{"type":"run-extension-command","commandName":"fixture","argumentValues":{"value":"saved"}}
            }]})).await
        })
    }
    fn launcher_search<'a>(
        &'a self,
        _ctx: &'a ExtensionContext,
        p: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            if p["text"] == "slow" {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Ok(
                json!({"rows":[{"id":"fixture:query","title":p["text"],"subtitle":"Dynamic item","score":280,
                "action":{"type":"run-extension-command","commandName":"fixture","argumentValues":{"value":p["text"]}}}]}),
            )
        })
    }
    fn background_activate<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        p: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            ctx.host(
                "host.background.retain",
                json!({"key":p["key"],"durable":true}),
            )
            .await
        })
    }
    fn background_event<'a>(
        &'a self,
        _ctx: &'a ExtensionContext,
        p: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move { Ok(p.clone()) })
    }
}
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(e) = serve(Fixture).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
