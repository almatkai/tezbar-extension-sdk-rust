use serde_json::json;
use tezbar_extension_sdk::*;
fn context(dir: &std::path::Path) -> TestContext {
    TestContext::new(
        ExtensionPaths {
            package_dir: dir.into(),
            data_dir: dir.join("data"),
            temp_dir: dir.join("temp"),
        },
        &["shell", "clipboard"],
    )
}
#[test]
fn encoders_match_browser_utf8_rules() {
    assert_eq!(
        encode_form_query("日本 😀 +~!*'()"),
        "%E6%97%A5%E6%9C%AC+%F0%9F%98%80+%2B%7E%21*%27%28%29"
    );
    assert_eq!(
        encode_component("日本 😀 +~!*'()"),
        "%E6%97%A5%E6%9C%AC%20%F0%9F%98%80%20%2B~!*'()"
    );
}
#[tokio::test]
async fn captures_typed_calls_rejects_domain_channels_and_preserves_errors() {
    let dir = tempfile::tempdir().unwrap();
    let test = context(dir.path());
    test.reply(Err("open failure".into()));
    assert_eq!(
        test.run(open(&test.context, "Қазақ")).await.unwrap_err(),
        "open failure"
    );
    assert_eq!(test.calls()[0]["method"], "host.shell.open");
    assert_eq!(test.calls()[0]["params"], json!({"target":"Қазақ"}));
    assert!(test
        .context
        .request("port-manager:named:add", vec![])
        .await
        .is_err());
    assert!(test.context.host("host.ai.ask", json!({})).await.is_err());
    test.context.emit("capture", json!("😀")).unwrap();
    assert_eq!(test.events()[0]["data"], "😀");
}
#[tokio::test]
async fn store_serializes_cross_context_updates_and_rejects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let a = context(dir.path());
    let b = context(dir.path());
    update_store(&a.context, "fixture", |v| {
        *v = json!({"unknown":{"keep":true},"count":0});
        Ok(())
    })
    .await
    .unwrap();
    let mut tasks = Vec::new();
    for i in 0..30 {
        let ctx = if i % 2 == 0 {
            a.context.clone()
        } else {
            b.context.clone()
        };
        tasks.push(tokio::spawn(async move {
            update_store(&ctx, "fixture", |v| {
                v["count"] = json!(v["count"].as_u64().unwrap() + 1);
                Ok(())
            })
            .await
            .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(
        read_store(&a.context, "fixture").await.unwrap(),
        json!({"unknown":{"keep":true},"count":30})
    );
    assert!(update_store(&a.context, "fixture", |v| {
        v["count"] = json!(99);
        Err("reject".into())
    })
    .await
    .is_err());
    assert_eq!(
        read_store(&a.context, "fixture").await.unwrap()["count"],
        30
    );
    let path = a.context.paths.data_dir.join("fixture/storage.json");
    std::fs::write(&path, b"{broken").unwrap();
    assert!(read_store(&a.context, "fixture").await.is_err());
    assert!(update_store(&a.context, "fixture", |_| Ok(()))
        .await
        .is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"{broken");
    for name in ["", "..", "../escape", "/absolute", "a/b", "a\\b"] {
        assert!(read_store(&a.context, name).await.is_err());
    }
}

#[tokio::test]
async fn dropping_runner_aborts_owned_tasks_and_closes_output() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    struct OnDrop(Arc<AtomicBool>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    struct Adapter(Arc<AtomicBool>);
    impl Extension for Adapter {
        fn id(&self) -> &'static str {
            "com.tezbar.native.drop"
        }
        fn version(&self) -> &'static str {
            "1.0.0"
        }
        fn commands(&self) -> &'static [&'static str] {
            &["drop"]
        }
        fn handle<'a>(
            &'a self,
            ctx: &'a ExtensionContext,
            _cmd: &'a str,
            op: &'a str,
            _p: &'a serde_json::Value,
            _state: &'a mut serde_json::Value,
        ) -> BoxFuture<'a, CoreResult> {
            Box::pin(async move {
                if op == "extension:run-command" {
                    let guard = OnDrop(self.0.clone());
                    ctx.spawn(async move {
                        let _guard = guard;
                        std::future::pending::<()>().await;
                    })?;
                }
                Ok(unchanged())
            })
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let (mut host_input, child_input) = tokio::io::duplex(8192);
    let (child_output, host_output) = tokio::io::duplex(8192);
    let runner = tokio::spawn(serve_io(
        Adapter(dropped.clone()),
        child_input,
        child_output,
    ));
    let mut output = BufReader::new(host_output);
    let mut line = String::new();
    output.read_line(&mut line).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    for (method, params) in [
        (
            "initialize",
            json!({"protocol":1,"extensionId":"com.tezbar.native.drop","platform":"test","packageDir":dir.path(),"dataDir":dir.path(),"tempDir":dir.path(),"grantedCapabilities":[]}),
        ),
        (
            "session.open",
            json!({"sessionId":"one","commandName":"drop"}),
        ),
    ] {
        let request = json!({"type":"request","id":format!("h:{}",uuid::Uuid::new_v4()),"method":method,"params":params});
        host_input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        line.clear();
        output.read_line(&mut line).await.unwrap();
    }
    runner.abort();
    let _ = runner.await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    line.clear();
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            output.read_line(&mut line)
        )
        .await
        .unwrap()
        .unwrap(),
        0
    );
}
