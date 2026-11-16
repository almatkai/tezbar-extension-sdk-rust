use serde_json::json;
use tezbar_extension_sdk::*;
#[tokio::test]
async fn quicklink_rpc_requires_grant_and_preserves_responses() {
    let denied = TestContext::new(ExtensionPaths::default(), &[]);
    assert!(denied
        .context
        .host("host.quicklinks.create", json!({"link":"pssword"}))
        .await
        .unwrap_err()
        .contains("not granted"));
    assert!(denied.calls().is_empty());
    let test = TestContext::new(ExtensionPaths::default(), &["host.quicklinks.create"]);
    for status in ["created", "cancelled"] {
        let reply = json!({"status":status});
        test.reply(Ok(reply.clone()));
        assert_eq!(
            test.run(
                test.context
                    .host("host.quicklinks.create", json!({"link":"pssword"}))
            )
            .await
            .unwrap(),
            reply
        );
        let call = test.calls().last().unwrap().clone();
        assert_eq!(call["method"], "host.quicklinks.create");
        assert_eq!(call["params"], json!({"link":"pssword"}));
    }
    test.reply(Err("Save failed".into()));
    assert_eq!(
        test.run(
            test.context
                .host("host.quicklinks.create", json!({"link":"pssword"}))
        )
        .await
        .unwrap_err(),
        "Save failed"
    );
    assert!(test
        .context
        .host_client
        .call("host.quicklinks.delete", json!({}))
        .await
        .unwrap_err()
        .contains("Unsupported host method"));
    assert_eq!(test.calls().len(), 3);
}
