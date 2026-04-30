use langshell::{LangShell, SideEffect};
use langshell_monty::MontyRuntime;
use serde_json::{Value, json};

#[tokio::main]
async fn main() {
    let shell = LangShell::builder()
        .runtime(MontyRuntime::new)
        .register_async(
            "fetch_json",
            "Example fetch_json capability.",
            SideEffect::Network,
            json!({"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 1}),
            json!({"type": "object", "properties": {"url": {"type": "string"}, "score": {"type": "number"}}, "required": ["url", "score"]}),
            |ctx| async move {
                let url = ctx.args.first().and_then(Value::as_str).unwrap_or_default();
                Ok(json!({"url": url, "score": 0.9}))
            },
        )
        .expect("register fetch_json")
        .build()
        .expect("build LangShell");

    let code = r#"
import asyncio
items = await asyncio.gather(*(fetch_json(f"https://api.example.com/i/{i}") for i in range(3)))
result = {"n": len(items), "scores": [item["score"] for item in items]}
"#;

    let result = shell.session("example-sdk").run(code).execute().await;
    println!("{}", serde_json::to_string_pretty(&result).expect("json"));
}
