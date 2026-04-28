use langshell::{LangShell, RunStatus, SideEffect};
use langshell_monty::MontyRuntime;
use serde_json::{Value, json};

#[tokio::test]
async fn sdk_registered_async_fetch_json_fans_out() {
    let shell = LangShell::builder()
        .runtime(MontyRuntime::new)
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
import asyncio
data = await asyncio.gather(*(fetch_json(f"https://api.example.com/i/{i}") for i in range(3)))
result = {"n": len(data), "ok": all(item["ok"] for item in data)}
"#;

    let result = shell.session("sdk-e2e").run(code).execute().await;
    assert_eq!(result.status, RunStatus::Ok, "{result:?}");
    assert_eq!(result.result, Some(json!({"n": 3, "ok": true})));
    assert_eq!(result.external_calls.len(), 3);
    assert_eq!(result.metrics.external_calls_count, 3);
}
