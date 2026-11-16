use serde_json::json;
use tezbar_extension_sdk::*;
#[tokio::test]
async fn session_services_require_negotiation_session_and_grants() {
    let mut test=TestContext::new(ExtensionPaths::default(),&[]);
    assert!(test.context.play_audio("x.mp3").await.unwrap_err().contains("not negotiated"));
    test.context.negotiated_features.insert("audio.local.v1".into());
    assert!(test.context.play_audio("x.mp3").await.unwrap_err().contains("session"));
    assert!(test.calls().is_empty());
    for method in ["host.audio.play","host.audio.stop","host.navigation.popToRoot"] {
        assert!(test.context.host(method,json!({"sessionId":"s"})).await.unwrap_err().contains("not granted"));
    }
}
#[tokio::test]
async fn generic_services_preserve_success_error_and_cancel_responses() {
    for method in ["host.audio.play","host.audio.stop","host.navigation.popToRoot"] {
        let test=TestContext::new(ExtensionPaths::default(),&[method]);
        let payload=json!({"sessionId":"synthetic","clearSearch":true});
        for result in [Ok(json!({"status":"accepted"})),Ok(json!({"status":"cancelled"})),Err("synthetic error".into())] {
            test.reply(result.clone());
            assert_eq!(test.run(test.context.host(method,payload.clone())).await,result);
            assert_eq!(test.calls().last().unwrap()["params"],payload);
        }
    }
}
