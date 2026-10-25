use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
#[tokio::test]
async fn post_ready_publication_negotiation_and_latest_query_are_real_pipes() {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sdk-fixture"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    let hello: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello["features"], json!(["launcher.v1"]));
    async fn send(input: &mut tokio::process::ChildStdin, method: &str, params: Value) -> String {
        let id = format!("h:{}", uuid::Uuid::new_v4());
        input
            .write_all(
                format!(
                    "{}\n",
                    json!({"type":"request","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        id
    }
    let id=send(&mut input,"initialize",json!({"protocol":1,"extensionId":"com.tezbar.native.fixture","platform":"fixture","packageDir":root.path(),"dataDir":root.path(),"tempDir":root.path(),"grantedCapabilities":["launcher.publish"],"hostFeatures":["launcher.v1"]})).await;
    let ready: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(ready["id"], id);
    assert_eq!(ready["result"]["features"], json!(["launcher.v1"]));
    let id = send(
        &mut input,
        "launcher.activate",
        json!({"contributionId":"items"}),
    )
    .await;
    let publish: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(publish["method"], "host.launcher.publish");
    assert_eq!(publish["params"]["rows"][0]["title"], "雪 café 😀");
    input
        .write_all(
            format!(
                "{}\n",
                json!({"type":"response","id":publish["id"],"result":{"published":true}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let activated: Value =
        serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(activated["id"], id);
    let slow=send(&mut input,"launcher.search",json!({"contributionId":"items","queryId":"old","text":"slow","surface":"query","limit":12})).await;
    let fast=send(&mut input,"launcher.search",json!({"contributionId":"items","queryId":"new","text":"alpha","surface":"query","limit":12})).await;
    let mut saw_old = false;
    let mut saw_new = false;
    for _ in 0..2 {
        let r: Value = serde_json::from_str(
            &tokio::time::timeout(Duration::from_millis(800), output.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        if r["id"] == slow {
            assert!(r.get("error").is_some());
            saw_old = true;
        } else {
            assert_eq!(r["id"], fast);
            assert_eq!(r["result"]["rows"][0]["title"], "alpha");
            saw_new = true;
        }
    }
    assert!(saw_old && saw_new);
    drop(input);
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
