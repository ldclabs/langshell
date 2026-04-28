use langshell::{LangShell, RunStatus, SideEffect};
use serde_json::{Value, json};

#[tokio::test]
async fn sdk_runs_typescript_session_state() {
    let shell = LangShell::builder().build().unwrap();

    let first = shell
        .typescript_session("ts-sdk-state")
        .run("cache = { k: 1 }")
        .execute()
        .await;
    assert_eq!(first.status, RunStatus::Ok, "{first:?}");

    let second = shell
        .typescript_session("ts-sdk-state")
        .run("result = cache.k + 1")
        .execute()
        .await;
    assert_eq!(second.status, RunStatus::Ok, "{second:?}");
    assert_eq!(second.result, Some(json!(2)));
}

#[tokio::test]
async fn sdk_typescript_awaits_registered_tool() {
    let shell = LangShell::builder()
        .register_async(
            "fetch_json",
            "Test fetch_json capability.",
            SideEffect::Network,
            |ctx| async move {
                let url = ctx.args.first().and_then(Value::as_str).unwrap_or_default();
                Ok(json!({"url": url, "ok": true}))
            },
        )
        .unwrap()
        .build()
        .unwrap();

    let code = r#"
data = await Promise.all([
  fetch_json("https://api.example.com/i/1"),
  fetch_json("https://api.example.com/i/2"),
  fetch_json("https://api.example.com/i/3"),
])
result = { n: data.length, ok: data.every((item) => item.ok) }
"#;

    let result = shell
        .typescript_session("ts-sdk-fanout")
        .run(code)
        .execute()
        .await;
    assert_eq!(result.status, RunStatus::Ok, "{result:?}");
    assert_eq!(result.result, Some(json!({"n": 3, "ok": true})));
    assert_eq!(result.external_calls.len(), 3);
    assert_eq!(result.metrics.external_calls_count, 3);
}
