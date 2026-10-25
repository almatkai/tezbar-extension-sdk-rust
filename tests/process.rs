use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
struct Process {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    _dir: tempfile::TempDir,
}
impl Process {
    async fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sdk-fixture"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut this = Self {
            child,
            input,
            output,
            _dir: tempfile::tempdir().unwrap(),
        };
        let hello = this.recv().await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["id"], "com.tezbar.native.fixture");
        this
    }
    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.output.read_line(&mut line))
                .await
                .expect("process reply timed out")
                .unwrap()
                > 0,
            "process EOF"
        );
        serde_json::from_str(&line).unwrap()
    }
    async fn send(&mut self, v: Value) {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(b'\n');
        self.input.as_mut().unwrap().write_all(&b).await.unwrap();
    }
    fn request(method: &str, params: Value) -> Value {
        json!({"type":"request","id":format!("h:{}",uuid::Uuid::new_v4()),"method":method,"params":params})
    }
    async fn invoke(&mut self, method: &str, params: Value) -> Value {
        let request = Self::request(method, params);
        self.send(request.clone()).await;
        let reply = self.recv().await;
        assert_eq!(reply["id"], request["id"]);
        reply
    }
    fn initialization(&self) -> Value {
        json!({"protocol":1,"extensionId":"com.tezbar.native.fixture","platform":std::env::consts::OS,"packageDir":self._dir.path(),"dataDir":self._dir.path().join("data"),"tempDir":self._dir.path().join("temp"),"grantedCapabilities":["shell","background"],"reason":"test","migrationPaths":{}})
    }
    async fn init(&mut self) {
        let p = self.initialization();
        assert_eq!(self.invoke("initialize", p).await["result"]["ready"], true);
    }
    async fn open(&mut self, id: &str) {
        assert_eq!(
            self.invoke(
                "session.open",
                json!({"sessionId":id,"commandName":"fixture"})
            )
            .await["result"]["title"],
            "Қазақ 😀"
        );
    }
    async fn finish(&mut self, success: bool) {
        drop(self.input.take());
        let status = tokio::time::timeout(Duration::from_secs(8), self.child.wait())
            .await
            .expect("process cleanup timed out")
            .unwrap();
        assert_eq!(status.success(), success);
    }
}
#[tokio::test]
async fn fragmented_unicode_and_identity() {
    let mut p = Process::new().await;
    let mut wrong = p.initialization();
    wrong["extensionId"] = json!("wrong");
    assert!(p.invoke("initialize", wrong).await.get("error").is_some());
    let request = Process::request("initialize", p.initialization());
    let mut bytes = serde_json::to_vec(&request).unwrap();
    bytes.push(b'\n');
    for chunk in bytes.chunks(3) {
        p.input.as_mut().unwrap().write_all(chunk).await.unwrap();
        tokio::task::yield_now().await;
    }
    assert_eq!(p.recv().await["result"]["ready"], true);
    let request = Process::request(
        "session.open",
        json!({"sessionId":"unicode 😀 日本", "commandName":"fixture"}),
    );
    let mut bytes = serde_json::to_vec(&request).unwrap();
    bytes.push(b'\n');
    for byte in bytes {
        p.input.as_mut().unwrap().write_all(&[byte]).await.unwrap();
        tokio::task::yield_now().await;
    }
    assert_eq!(p.recv().await["result"]["title"], "Қазақ 😀");
    assert!(p
        .invoke(
            "session.open",
            json!({"sessionId":"bad","commandName":"unknown"})
        )
        .await
        .get("error")
        .is_some());
    p.finish(true).await;
}
#[tokio::test]
async fn concurrent_reverse_rpc_and_session_isolation() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    p.open("two").await;
    let one = Process::request(
        "session.call",
        json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"reverse","value":"日本 😀"}}),
    );
    let two = Process::request(
        "session.call",
        json!({"sessionId":"two","operation":"extension:invoke-action","payload":{"actionId":"reverse","value":"Қазақ"}}),
    );
    p.send(one.clone()).await;
    p.send(two.clone()).await;
    let a = p.recv().await;
    let b = p.recv().await;
    assert_eq!(a["method"], "host.shell.open");
    assert_eq!(b["method"], "host.shell.open");
    for v in [&b, &a] {
        p.send(json!({"type":"response","id":v["id"],"result":v["params"]["target"]}))
            .await;
    }
    let replies = [p.recv().await, p.recv().await];
    assert_eq!(
        replies.iter().find(|v| v["id"] == one["id"]).unwrap()["result"],
        "日本 😀"
    );
    assert_eq!(
        replies.iter().find(|v| v["id"] == two["id"]).unwrap()["result"],
        "Қазақ"
    );
    p.finish(true).await;
}
#[tokio::test]
async fn cancel_reverse_call_and_busy_refresh() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    let request = Process::request(
        "session.call",
        json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"reverse","value":"hold"}}),
    );
    p.send(request.clone()).await;
    let reverse = p.recv().await;
    assert_eq!(
        p.invoke(
            "session.call",
            json!({"sessionId":"one","operation":"extension:refresh-session"})
        )
        .await["result"]["mode"],
        "unchanged"
    );
    p.send(json!({"type":"cancel","requestId":request["id"],"reason":"test"}))
        .await;
    let a = p.recv().await;
    let b = p.recv().await;
    let frames = [a, b];
    assert!(frames
        .iter()
        .any(|v| v["type"] == "cancel" && v["requestId"] == reverse["id"]));
    assert!(frames
        .iter()
        .any(|v| v["id"] == request["id"] && v.get("error").is_some()));
    // Late reverse response is discarded and must not resurrect operation state.
    p.send(json!({"type":"response","id":reverse["id"],"result":"late"}))
        .await;
    p.finish(true).await;
}
#[tokio::test]
async fn stale_search_never_commits_and_dispose_fences_tasks() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    p.open("two").await;
    let slow = Process::request(
        "session.call",
        json!({"sessionId":"one","operation":"extension:search-text-changed","payload":{"searchText":"slow"}}),
    );
    p.send(slow.clone()).await;
    let reverse = p.recv().await;
    let fast = Process::request(
        "session.call",
        json!({"sessionId":"one","operation":"extension:search-text-changed","payload":{"searchText":"最新 😀"}}),
    );
    p.send(fast.clone()).await;
    let mut saw_fast = false;
    let mut saw_cancel = false;
    let mut saw_slow = false;
    for _ in 0..3 {
        let v = p.recv().await;
        if v["id"] == fast["id"] {
            assert_eq!(v["result"]["query"], "最新 😀");
            saw_fast = true;
        }
        if v["id"] == slow["id"] {
            assert!(v.get("error").is_some());
            saw_slow = true;
        }
        if v["requestId"] == reverse["id"] {
            saw_cancel = true;
        }
    }
    assert!(saw_fast && saw_cancel && saw_slow);
    assert_eq!(
        p.invoke(
            "session.call",
            json!({"sessionId":"one","operation":"extension:invoke-action","payload":{}})
        )
        .await["result"]["query"],
        "最新 😀"
    );
    assert_eq!(
        p.invoke(
            "session.call",
            json!({"sessionId":"two","operation":"extension:invoke-action","payload":{}})
        )
        .await["result"],
        json!({})
    );
    p.invoke("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"spawn"}})).await;
    assert!(p
        .invoke("session.dispose", json!({"sessionId":"one"}))
        .await
        .get("result")
        .is_some());
    tokio::time::sleep(Duration::from_millis(180)).await;
    // Any late event would arrive before this response and fail invoke's correlation check.
    assert!(p
        .invoke(
            "session.call",
            json!({"sessionId":"one","operation":"extension:refresh-session"})
        )
        .await
        .get("error")
        .is_some());
    p.finish(true).await;
}
#[tokio::test]
async fn lifecycle_events_durable_lease_and_shutdown() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    for revision in [1, 2] {
        let request = Process::request(
            "session.call",
            json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"invalidate"}}),
        );
        p.send(request).await;
        let event = p.recv().await;
        assert_eq!(event["event"], "session.invalidated");
        assert_eq!(event["sessionId"], "one");
        assert_eq!(event["revision"], revision);
        assert!(p.recv().await.get("result").is_some());
    }
    let request = Process::request("background.activate", json!({"key":"timer"}));
    p.send(request.clone()).await;
    let reverse = p.recv().await;
    assert_eq!(reverse["method"], "host.background.retain");
    assert_eq!(reverse["params"]["durable"], true);
    p.send(json!({"type":"response","id":reverse["id"],"result":{"retained":true}}))
        .await;
    assert_eq!(p.recv().await["id"], request["id"]);
    let params = json!({"resourceId":"tray","actionId":"click","data":"✓"});
    assert_eq!(
        p.invoke("background.event", params.clone()).await["result"],
        params
    );
    assert!(p
        .invoke("shutdown", json!({}))
        .await
        .get("result")
        .is_some());
    // Keep host stdin open: shutdown must exit without requiring parent EOF.
    assert!(tokio::time::timeout(Duration::from_secs(2), p.child.wait())
        .await
        .expect("shutdown waits for stdin EOF")
        .unwrap()
        .success());
    p.finish(true).await;
}
#[tokio::test]
async fn malformed_crlf_and_truncated_are_fatal() {
    for bytes in [
        b"{broken}\n".as_slice(),
        b"{}\r\n".as_slice(),
        b"{\"type\":\"request\"}".as_slice(),
    ] {
        let mut p = Process::new().await;
        p.init().await;
        p.open("one").await;
        p.input.as_mut().unwrap().write_all(bytes).await.unwrap();
        p.finish(false).await;
    }
}
#[tokio::test]
async fn oversize_frame_is_bounded_and_fatal() {
    let mut p = Process::new().await;
    let bytes = vec![b'x'; tezbar_extension_sdk::MAX_FRAME + 1];
    // The child may close the read end immediately once the limit is reached.
    let _ = p.input.as_mut().unwrap().write_all(&bytes).await;
    p.finish(false).await;
}
#[tokio::test]
async fn eof_with_pending_reverse_rpc_does_not_hang() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    p.send(Process::request("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"reverse"}}))).await;
    assert_eq!(p.recv().await["type"], "request");
    p.finish(true).await;
}
#[tokio::test]
async fn session_limit_and_duplicate_request_are_bounded() {
    let mut p = Process::new().await;
    p.init().await;
    for i in 0..128 {
        p.open(&i.to_string()).await;
    }
    assert!(p
        .invoke(
            "session.open",
            json!({"sessionId":"overflow","commandName":"fixture"})
        )
        .await
        .get("error")
        .is_some());
    let request = Process::request(
        "session.call",
        json!({"sessionId":"0","operation":"extension:invoke-action","payload":{"actionId":"reverse"}}),
    );
    p.send(request.clone()).await;
    p.recv().await;
    p.send(request).await;
    p.finish(false).await;
}

#[tokio::test]
async fn partial_utf8_frame_survives_another_handler_finishing() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    let delay = Process::request(
        "session.call",
        json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"delay"}}),
    );
    p.send(delay.clone()).await;
    let request = Process::request(
        "session.call",
        json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"value":"日本 😀"}}),
    );
    let mut bytes = serde_json::to_vec(&request).unwrap();
    bytes.push(b'\n');
    let split = bytes.iter().position(|byte| *byte >= 128).unwrap() + 1;
    p.input
        .as_mut()
        .unwrap()
        .write_all(&bytes[..split])
        .await
        .unwrap();
    assert_eq!(p.recv().await["id"], delay["id"]);
    tokio::time::sleep(Duration::from_millis(30)).await;
    p.input
        .as_mut()
        .unwrap()
        .write_all(&bytes[split..])
        .await
        .unwrap();
    assert_eq!(p.recv().await["id"], request["id"]);
    p.finish(true).await;
}
#[tokio::test]
async fn oversized_handler_result_is_an_error_frame_not_oversized_output() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    let reply=p.invoke("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"large"}})).await;
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("16 MiB"));
    p.finish(true).await;
}

#[tokio::test]
async fn bounded_handler_and_scoped_task_limits_recover_after_disposal() {
    let mut p = Process::new().await;
    p.init().await;
    p.open("one").await;
    assert_eq!(p.invoke("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"scope-limit"}})).await["result"]["accepted"],64);
    assert_eq!(p.invoke("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"scope-limit"}})).await["result"]["accepted"],0);
    p.invoke("session.dispose", json!({"sessionId":"one"}))
        .await;
    p.open("one").await;
    assert_eq!(p.invoke("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"scope-limit"}})).await["result"]["accepted"],64);
    p.send(Process::request("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"reverse"}}))).await;
    assert_eq!(p.recv().await["type"], "request");
    for _ in 1..120 {
        p.send(Process::request("session.call",json!({"sessionId":"one","operation":"extension:invoke-action","payload":{"actionId":"reverse"}}))).await;
    }
    let reply = p
        .invoke(
            "session.call",
            json!({"sessionId":"one","operation":"extension:invoke-action","payload":{}}),
        )
        .await;
    assert!(reply["error"]["message"].as_str().unwrap().contains("busy"));
    let shutdown = Process::request("shutdown", json!({}));
    p.send(shutdown.clone()).await;
    let mut acknowledged = false;
    for _ in 0..125 {
        let reply = p.recv().await;
        if reply["id"] == shutdown["id"] {
            assert!(reply.get("result").is_some());
            acknowledged = true;
            break;
        }
    }
    assert!(
        acknowledged,
        "shutdown must work with saturated action slots"
    );
    p.finish(true).await;
}
