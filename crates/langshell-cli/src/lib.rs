use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use clap::{Args, Parser, Subcommand};
use langshell::{
    ErrorObject, LangShell, Language, RunRequest, RunResult, RunStatus, SessionId, SessionLimits,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

#[derive(Parser, Debug)]
#[command(
    name = "langshell",
    version,
    about = "Stateful sandboxed Python execution for AI agents"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run(RunCommand),
    Validate(RunCommand),
    Repl(ReplCommand),
    Daemon(DaemonCommand),
    Session(SessionCommand),
    Tools(ToolsCommand),
}

#[derive(Args, Debug, Clone)]
struct RunCommand {
    #[arg(short = 'e', long = "eval", conflicts_with = "file")]
    eval: Option<String>,
    #[arg(short = 'f', long = "file", conflicts_with = "eval")]
    file: Option<PathBuf>,
    #[arg(long = "session-id", default_value = "default")]
    session_id: String,
    #[arg(long = "timeout")]
    timeout_ms: Option<u32>,
    #[arg(long = "json", default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug)]
struct ReplCommand {
    #[arg(long = "session-id", default_value = "default")]
    session_id: String,
}

#[derive(Args, Debug)]
struct DaemonCommand {
    #[arg(long = "listen")]
    listen: String,
}

#[derive(Subcommand, Debug)]
enum SessionAction {
    List,
    Snapshot {
        session_id: String,
        #[arg(long = "out")]
        out: PathBuf,
    },
    Restore {
        #[arg(long = "from")]
        from: PathBuf,
        #[arg(long = "session-id")]
        session_id: String,
    },
    Destroy {
        session_id: String,
    },
}

#[derive(Args, Debug)]
struct SessionCommand {
    #[command(subcommand)]
    action: SessionAction,
}

#[derive(Subcommand, Debug)]
enum ToolsAction {
    List {
        #[arg(long = "session-id", default_value = "default")]
        session_id: String,
    },
    Describe {
        name: String,
        #[arg(long = "session-id", default_value = "default")]
        session_id: String,
    },
}

#[derive(Args, Debug)]
struct ToolsCommand {
    #[command(subcommand)]
    action: ToolsAction,
}

pub async fn run() -> ExitCode {
    match run_inner(Cli::parse()).await {
        Ok(code) => code,
        Err(error) => {
            let result = RunResult::error(
                langshell::RunStatus::RuntimeError,
                error,
                String::new(),
                Default::default(),
            );
            let _ = print_json(&result);
            ExitCode::FAILURE
        }
    }
}

async fn run_inner(cli: Cli) -> Result<ExitCode, ErrorObject> {
    match cli.command {
        Command::Run(command) => run_code(command, false).await,
        Command::Validate(command) => run_code(command, true).await,
        Command::Repl(command) => run_repl(command).await,
        Command::Daemon(command) => run_daemon(command).await,
        Command::Session(command) => run_session_command(command).await,
        Command::Tools(command) => run_tools_command(command).await,
    }
}

async fn run_code(command: RunCommand, validate_only: bool) -> Result<ExitCode, ErrorObject> {
    let shell = default_shell()?;
    if !validate_only {
        load_session_if_exists(&shell, &command.session_id).await?;
    }
    let code = read_code(&command)?;
    let mut request = RunRequest::new(&command.session_id, code)?;
    request.validate_only = validate_only;
    request.timeout_ms = command.timeout_ms;
    let result = shell.run(request).await;
    if !validate_only && result.status == RunStatus::Ok {
        save_session(&shell, &command.session_id).await?;
    }
    print_json(&result)?;
    Ok(if result.status == RunStatus::Ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

async fn run_repl(command: ReplCommand) -> Result<ExitCode, ErrorObject> {
    let shell = default_shell()?;
    load_session_if_exists(&shell, &command.session_id).await?;
    let mut line = String::new();
    loop {
        print!("langshell:{}> ", command.session_id);
        io::stdout()
            .flush()
            .map_err(|err| ErrorObject::new("IO_ERROR", format!("stdout: {err}")))?;
        line.clear();
        let bytes = io::stdin()
            .read_line(&mut line)
            .map_err(|err| ErrorObject::new("IO_ERROR", format!("stdin: {err}")))?;
        if bytes == 0 || matches!(line.trim(), "exit" | "quit") {
            break;
        }
        let result = shell
            .session(&command.session_id)
            .run(line.clone())
            .execute()
            .await;
        print_json(&result)?;
        if result.status == RunStatus::Ok {
            save_session(&shell, &command.session_id).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_session_command(command: SessionCommand) -> Result<ExitCode, ErrorObject> {
    let shell = default_shell()?;
    match command.action {
        SessionAction::List => {
            let ids = list_stored_sessions()?;
            print_json(&json!({"status": "ok", "sessions": ids}))?;
        }
        SessionAction::Snapshot { session_id, out } => {
            load_session_if_exists(&shell, &session_id).await?;
            let snapshot = shell.snapshot_session(&session_id).await?;
            fs::write(&out, snapshot).map_err(|err| {
                ErrorObject::new("IO_ERROR", format!("writing {}: {err}", out.display()))
            })?;
            print_json(&json!({"status": "ok", "out": out}))?;
        }
        SessionAction::Restore { from, session_id } => {
            let bytes = fs::read(&from).map_err(|err| {
                ErrorObject::new("IO_ERROR", format!("reading {}: {err}", from.display()))
            })?;
            shell
                .restore_session(&bytes, Some(session_id.clone()))
                .await?;
            save_session(&shell, &session_id).await?;
            print_json(&json!({"status": "ok", "session_id": session_id}))?;
        }
        SessionAction::Destroy { session_id } => {
            let path = session_file(&session_id)?;
            let removed = if path.exists() {
                fs::remove_file(&path).map_err(|err| {
                    ErrorObject::new("IO_ERROR", format!("removing {}: {err}", path.display()))
                })?;
                true
            } else {
                false
            };
            print_json(&json!({"status": "ok", "removed": removed}))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_tools_command(command: ToolsCommand) -> Result<ExitCode, ErrorObject> {
    let shell = default_shell()?;
    match command.action {
        ToolsAction::List { session_id } => {
            load_session_if_exists(&shell, &session_id).await?;
            let result = shell
                .session(&session_id)
                .run("result = list_tools()")
                .execute()
                .await;
            print_json(&result)?;
        }
        ToolsAction::Describe { name, session_id } => {
            load_session_if_exists(&shell, &session_id).await?;
            let code = format!("result = describe_tool({name:?})");
            let result = shell.session(&session_id).run(code).execute().await;
            print_json(&result)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_daemon(command: DaemonCommand) -> Result<ExitCode, ErrorObject> {
    let path = command.listen.strip_prefix("unix://").ok_or_else(|| {
        ErrorObject::new(
            "INVALID_ARGUMENT",
            "Only unix:// daemon listeners are supported in MVP.",
        )
    })?;
    if Path::new(path).exists() {
        fs::remove_file(path).map_err(|err| {
            ErrorObject::new(
                "IO_ERROR",
                format!("removing existing socket {path}: {err}"),
            )
        })?;
    }
    let listener = UnixListener::bind(path)
        .map_err(|err| ErrorObject::new("IO_ERROR", format!("binding {path}: {err}")))?;
    let shell = default_shell()?;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|err| ErrorObject::new("IO_ERROR", format!("accept: {err}")))?;
        let shell = shell.clone();
        tokio::spawn(async move {
            let _ = handle_rpc_stream(shell, stream).await;
        });
    }
}

async fn handle_rpc_stream(shell: LangShell, stream: UnixStream) -> Result<(), ErrorObject> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|err| ErrorObject::new("IO_ERROR", format!("reading rpc line: {err}")))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_rpc_request(&shell, &line).await;
        writer
            .write_all(response.to_string().as_bytes())
            .await
            .map_err(|err| ErrorObject::new("IO_ERROR", format!("writing rpc response: {err}")))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|err| ErrorObject::new("IO_ERROR", format!("writing rpc newline: {err}")))?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

async fn handle_rpc_request(shell: &LangShell, line: &str) -> Value {
    let request: RpcRequest = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(err) => {
            return json!(RpcResponse {
                jsonrpc: "2.0",
                id: Value::Null,
                result: None,
                error: Some(json!({"code": -32700, "message": err.to_string()})),
            });
        }
    };

    let id = request.id.clone();
    let result = match request.method.as_str() {
        "session.create" => rpc_session_create(shell, request.params).await,
        "session.run" => rpc_session_run(shell, request.params, false).await,
        "session.validate" => rpc_session_run(shell, request.params, true).await,
        "session.snapshot" => rpc_session_snapshot(shell, request.params).await,
        "session.restore" => rpc_session_restore(shell, request.params).await,
        "session.destroy" => rpc_session_destroy(shell, request.params).await,
        "tools.list" => rpc_tools_list(shell, request.params).await,
        "tools.describe" => rpc_tools_describe(shell, request.params).await,
        "policy.get" => rpc_policy_get(shell, request.params).await,
        other => Err(ErrorObject::new(
            "METHOD_NOT_FOUND",
            format!("Unknown method {other}."),
        )),
    };

    match result {
        Ok(value) => json!(RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(value),
            error: None,
        }),
        Err(error) => json!(RpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(json!({"code": -32000, "message": error.message, "data": error})),
        }),
    }
}

async fn rpc_session_create(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    shell.create_session(&session_id).await?;
    Ok(json!({"status": "ok", "session_id": session_id}))
}

async fn rpc_session_run(
    shell: &LangShell,
    params: Value,
    validate_only: bool,
) -> Result<Value, ErrorObject> {
    let mut request = request_from_params(params)?;
    request.validate_only = validate_only;
    let result = shell.run(request).await;
    Ok(json!(result))
}

async fn rpc_session_snapshot(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    let snapshot = shell.snapshot_session(&session_id).await?;
    Ok(json!({
        "status": "ok",
        "snapshot_id": format!("snap_{}", session_id),
        "snapshot": BASE64.encode(snapshot),
    }))
}

async fn rpc_session_restore(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let snapshot_b64 = string_param(&params, "snapshot").ok_or_else(|| {
        ErrorObject::new("INVALID_ARGUMENT", "session.restore requires snapshot.")
    })?;
    let snapshot = BASE64
        .decode(snapshot_b64)
        .map_err(|err| ErrorObject::new("SNAPSHOT_CORRUPT", format!("base64: {err}")))?;
    let session_id = string_param(&params, "session_id");
    let restored = shell.restore_session(&snapshot, session_id).await?;
    Ok(json!({"status": "ok", "session_id": restored.0}))
}

async fn rpc_session_destroy(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    let removed = shell.destroy_session(&session_id).await?;
    Ok(json!({"status": "ok", "removed": removed}))
}

async fn rpc_tools_list(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    let result = shell
        .session(session_id)
        .run("result = list_tools()")
        .execute()
        .await;
    Ok(json!(result))
}

async fn rpc_tools_describe(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    let name = string_param(&params, "name")
        .or_else(|| string_param(&params, "tool"))
        .ok_or_else(|| ErrorObject::new("INVALID_ARGUMENT", "tools.describe requires name."))?;
    let result = shell
        .session(session_id)
        .run(format!("result = describe_tool({name:?})"))
        .execute()
        .await;
    Ok(json!(result))
}

async fn rpc_policy_get(shell: &LangShell, params: Value) -> Result<Value, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    let result = shell
        .session(session_id)
        .run("result = current_policy()")
        .execute()
        .await;
    Ok(json!(result))
}

fn request_from_params(params: Value) -> Result<RunRequest, ErrorObject> {
    let session_id = string_param(&params, "session_id").unwrap_or_else(|| "default".to_owned());
    let code = string_param(&params, "code")
        .ok_or_else(|| ErrorObject::new("INVALID_ARGUMENT", "session.run requires code."))?;
    let mut request = RunRequest::new(session_id, code)?;
    request.language = match string_param(&params, "language").as_deref() {
        Some("python") | Some("Python") | None => Language::Python,
        Some("typescript") | Some("TypeScript") => Language::TypeScript,
        Some(other) => {
            return Err(ErrorObject::new(
                "UNSUPPORTED_FEATURE",
                format!("Language {other} is not supported in MVP."),
            ));
        }
    };
    if let Some(inputs) = params.get("inputs").and_then(Value::as_object) {
        request.inputs = inputs.clone();
    }
    request.timeout_ms = params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    request.return_snapshot = params
        .get("return_snapshot")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(limits) = params.get("limits") {
        request.limits = Some(
            serde_json::from_value::<SessionLimits>(limits.clone()).map_err(|err| {
                ErrorObject::new("INVALID_ARGUMENT", format!("invalid limits: {err}"))
            })?,
        );
    }
    Ok(request)
}

fn string_param(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn default_shell() -> Result<LangShell, ErrorObject> {
    LangShell::builder().build()
}

async fn load_session_if_exists(shell: &LangShell, session_id: &str) -> Result<(), ErrorObject> {
    let path = session_file(session_id)?;
    if path.exists() {
        let bytes = fs::read(&path).map_err(|err| {
            ErrorObject::new("IO_ERROR", format!("reading {}: {err}", path.display()))
        })?;
        shell
            .restore_session(&bytes, Some(session_id.to_owned()))
            .await?;
    }
    Ok(())
}

async fn save_session(shell: &LangShell, session_id: &str) -> Result<(), ErrorObject> {
    let bytes = shell.snapshot_session(session_id).await?;
    let path = session_file(session_id)?;
    let parent = path.parent().expect("session file has parent");
    fs::create_dir_all(parent).map_err(|err| {
        ErrorObject::new("IO_ERROR", format!("creating {}: {err}", parent.display()))
    })?;
    fs::write(&path, bytes)
        .map_err(|err| ErrorObject::new("IO_ERROR", format!("writing {}: {err}", path.display())))
}

fn list_stored_sessions() -> Result<Vec<String>, ErrorObject> {
    let dir = session_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(&dir)
        .map_err(|err| ErrorObject::new("IO_ERROR", format!("reading {}: {err}", dir.display())))?
    {
        let entry = entry
            .map_err(|err| ErrorObject::new("IO_ERROR", format!("reading session entry: {err}")))?;
        if let Some(name) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
        {
            sessions.push(name.to_owned());
        }
    }
    sessions.sort();
    Ok(sessions)
}

fn session_file(session_id: &str) -> Result<PathBuf, ErrorObject> {
    let session_id = SessionId::new(session_id)?;
    Ok(session_dir().join(format!("{}.json", session_id.0)))
}

fn session_dir() -> PathBuf {
    std::env::var_os("LANGSHELL_SESSION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("langshell").join("sessions"))
}

fn read_code(command: &RunCommand) -> Result<String, ErrorObject> {
    match (&command.eval, &command.file) {
        (Some(code), None) => Ok(code.clone()),
        (None, Some(path)) => fs::read_to_string(path).map_err(|err| {
            ErrorObject::new("IO_ERROR", format!("reading {}: {err}", path.display()))
        }),
        (None, None) => Err(ErrorObject::new(
            "INVALID_ARGUMENT",
            "Provide code with -e or -f.",
        )),
        (Some(_), Some(_)) => Err(ErrorObject::new(
            "INVALID_ARGUMENT",
            "Use either -e or -f, not both.",
        )),
    }
}

fn print_json(value: &impl Serialize) -> Result<(), ErrorObject> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| ErrorObject::new("SERIALIZE_ERROR", format!("json output: {err}")))?;
    println!("{text}");
    Ok(())
}
