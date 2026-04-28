use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use deno_ast::{MediaType, ParseParams, SourceMapOption};
use deno_core::{Extension, JsRuntime, OpState, PollEventLoopOptions, RuntimeOptions, op2, v8};
use deno_error::JsErrorBox;
use langshell_core::{
    CallStatus, ErrorObject, ExternalCallRecord, Language, LanguageRuntime, Metrics, RunRequest,
    RunResult, RunStatus, RuntimeFuture, SessionId, SessionLimits, ToolCallContext, ToolRegistry,
    digest_bytes, digest_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

pub const DENO_SNAPSHOT_MAGIC: &str = "langshell-deno-snapshot/v1";

#[derive(Clone)]
pub struct DenoRuntime {
    tx: mpsc::UnboundedSender<DenoCommand>,
}

impl fmt::Debug for DenoRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DenoRuntime").finish_non_exhaustive()
    }
}

impl DenoRuntime {
    pub fn new(registry: ToolRegistry, default_limits: SessionLimits) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("langshell-deno".to_owned())
            .spawn(move || run_worker_thread(rx, registry, default_limits))
            .expect("failed to spawn langshell-deno worker thread");
        Self { tx }
    }

    pub async fn create_session(
        &self,
        session_id: SessionId,
        limits: Option<SessionLimits>,
    ) -> Result<(), ErrorObject> {
        let (reply, rx) = oneshot::channel();
        self.send(DenoCommand::CreateSession {
            session_id,
            limits,
            reply,
        })?;
        rx.await.unwrap_or_else(|_| Err(worker_closed_error()))
    }

    pub async fn run(&self, request: RunRequest) -> RunResult {
        let (reply, rx) = oneshot::channel();
        if let Err(error) = self.send(DenoCommand::Run { request, reply }) {
            return runtime_error_result(error);
        }
        rx.await
            .unwrap_or_else(|_| runtime_error_result(worker_closed_error()))
    }

    pub async fn destroy_session(&self, session_id: &SessionId) -> Result<bool, ErrorObject> {
        let (reply, rx) = oneshot::channel();
        self.send(DenoCommand::DestroySession {
            session_id: session_id.clone(),
            reply,
        })?;
        rx.await.map_err(|_| worker_closed_error())
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionId>, ErrorObject> {
        let (reply, rx) = oneshot::channel();
        self.send(DenoCommand::ListSessions { reply })?;
        rx.await.map_err(|_| worker_closed_error())
    }

    pub async fn snapshot_session(&self, session_id: &SessionId) -> Result<Vec<u8>, ErrorObject> {
        let (reply, rx) = oneshot::channel();
        self.send(DenoCommand::SnapshotSession {
            session_id: session_id.clone(),
            reply,
        })?;
        rx.await.unwrap_or_else(|_| Err(worker_closed_error()))
    }

    pub async fn restore_session(
        &self,
        snapshot: &[u8],
        session_id: Option<SessionId>,
    ) -> Result<SessionId, ErrorObject> {
        let (reply, rx) = oneshot::channel();
        self.send(DenoCommand::RestoreSession {
            snapshot: snapshot.to_vec(),
            session_id,
            reply,
        })?;
        rx.await.unwrap_or_else(|_| Err(worker_closed_error()))
    }

    fn send(&self, command: DenoCommand) -> Result<(), ErrorObject> {
        self.tx.send(command).map_err(|_| worker_closed_error())
    }
}

impl LanguageRuntime for DenoRuntime {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn create_session(
        &self,
        session_id: SessionId,
        limits: Option<SessionLimits>,
    ) -> RuntimeFuture<'_, Result<(), ErrorObject>> {
        Box::pin(async move { DenoRuntime::create_session(self, session_id, limits).await })
    }

    fn run(&self, request: RunRequest) -> RuntimeFuture<'_, RunResult> {
        Box::pin(async move { DenoRuntime::run(self, request).await })
    }

    fn destroy_session(
        &self,
        session_id: SessionId,
    ) -> RuntimeFuture<'_, Result<bool, ErrorObject>> {
        Box::pin(async move { DenoRuntime::destroy_session(self, &session_id).await })
    }

    fn list_sessions(&self) -> RuntimeFuture<'_, Result<Vec<SessionId>, ErrorObject>> {
        Box::pin(async move { DenoRuntime::list_sessions(self).await })
    }

    fn snapshot_session(
        &self,
        session_id: SessionId,
    ) -> RuntimeFuture<'_, Result<Vec<u8>, ErrorObject>> {
        Box::pin(async move { DenoRuntime::snapshot_session(self, &session_id).await })
    }

    fn restore_session(
        &self,
        snapshot: Vec<u8>,
        session_id: Option<SessionId>,
    ) -> RuntimeFuture<'_, Result<SessionId, ErrorObject>> {
        Box::pin(async move { DenoRuntime::restore_session(self, &snapshot, session_id).await })
    }

    fn can_restore_snapshot(&self, snapshot: &[u8]) -> bool {
        is_deno_snapshot(snapshot)
    }
}

pub fn is_deno_snapshot(snapshot: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(snapshot)
        .ok()
        .and_then(|value| {
            value
                .get("magic")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some(DENO_SNAPSHOT_MAGIC)
}

enum DenoCommand {
    CreateSession {
        session_id: SessionId,
        limits: Option<SessionLimits>,
        reply: oneshot::Sender<Result<(), ErrorObject>>,
    },
    Run {
        request: RunRequest,
        reply: oneshot::Sender<RunResult>,
    },
    DestroySession {
        session_id: SessionId,
        reply: oneshot::Sender<bool>,
    },
    ListSessions {
        reply: oneshot::Sender<Vec<SessionId>>,
    },
    SnapshotSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<Vec<u8>, ErrorObject>>,
    },
    RestoreSession {
        snapshot: Vec<u8>,
        session_id: Option<SessionId>,
        reply: oneshot::Sender<Result<SessionId, ErrorObject>>,
    },
}

fn run_worker_thread(
    mut rx: mpsc::UnboundedReceiver<DenoCommand>,
    registry: ToolRegistry,
    default_limits: SessionLimits,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build langshell-deno tokio runtime");
    runtime.block_on(async move {
        let mut worker = DenoWorker::new(registry, default_limits);
        while let Some(command) = rx.recv().await {
            worker.handle(command).await;
        }
    });
}

struct DenoWorker {
    sessions: HashMap<String, DenoSession>,
    registry: ToolRegistry,
    default_limits: SessionLimits,
}

impl DenoWorker {
    fn new(registry: ToolRegistry, default_limits: SessionLimits) -> Self {
        Self {
            sessions: HashMap::new(),
            registry,
            default_limits,
        }
    }

    async fn handle(&mut self, command: DenoCommand) {
        match command {
            DenoCommand::CreateSession {
                session_id,
                limits,
                reply,
            } => {
                let result = self.create_session(session_id, limits);
                let _ = reply.send(result);
            }
            DenoCommand::Run { request, reply } => {
                let result = self.run(request).await;
                let _ = reply.send(result);
            }
            DenoCommand::DestroySession { session_id, reply } => {
                let _ = reply.send(self.sessions.remove(&session_id.0).is_some());
            }
            DenoCommand::ListSessions { reply } => {
                let mut ids: Vec<_> = self.sessions.keys().cloned().map(SessionId).collect();
                ids.sort_by(|a, b| a.0.cmp(&b.0));
                let _ = reply.send(ids);
            }
            DenoCommand::SnapshotSession { session_id, reply } => {
                let result = self.snapshot_session(&session_id);
                let _ = reply.send(result);
            }
            DenoCommand::RestoreSession {
                snapshot,
                session_id,
                reply,
            } => {
                let result = self.restore_session(&snapshot, session_id);
                let _ = reply.send(result);
            }
        }
    }

    fn create_session(
        &mut self,
        session_id: SessionId,
        limits: Option<SessionLimits>,
    ) -> Result<(), ErrorObject> {
        if !self.sessions.contains_key(&session_id.0) {
            let limits = limits.unwrap_or_else(|| self.default_limits.clone());
            let session = DenoSession::new(session_id.clone(), limits, &self.registry)?;
            self.sessions.insert(session_id.0, session);
        }
        Ok(())
    }

    async fn run(&mut self, request: RunRequest) -> RunResult {
        if request.language != Language::TypeScript {
            return RunResult::error(
                RunStatus::ValidationError,
                ErrorObject::new(
                    "UNSUPPORTED_FEATURE",
                    "The Deno backend only executes TypeScript.",
                ),
                String::new(),
                Metrics::default(),
            );
        }
        if request.validate_only {
            return validate_request(&request, &self.registry);
        }

        let limits = effective_limits(&self.default_limits, &request);
        let session_id = request.session_id.clone();
        let mut session = match self.sessions.remove(&session_id.0) {
            Some(mut session) => {
                session.limits = limits;
                session
            }
            None => match DenoSession::new(session_id.clone(), limits, &self.registry) {
                Ok(session) => session,
                Err(error) => {
                    return RunResult::error(
                        RunStatus::RuntimeError,
                        error,
                        String::new(),
                        Metrics::default(),
                    );
                }
            },
        };

        let result = session.run(request, &self.registry).await;
        self.sessions.insert(session_id.0, session);
        result
    }

    fn snapshot_session(&mut self, session_id: &SessionId) -> Result<Vec<u8>, ErrorObject> {
        let session = self.sessions.get_mut(&session_id.0).ok_or_else(|| {
            ErrorObject::new(
                "SESSION_NOT_FOUND",
                format!("TypeScript session {} does not exist.", session_id.0),
            )
        })?;
        let snapshot = SnapshotEnvelope {
            magic: DENO_SNAPSHOT_MAGIC.to_owned(),
            version: langshell_core::SNAPSHOT_VERSION,
            session_id: session_id.0.clone(),
            limits: session.limits.clone(),
            globals: session.snapshot_globals()?,
            capability_digest: capability_digest(&self.registry),
        };
        serde_json::to_vec(&snapshot).map_err(|err| {
            ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                format!("Failed to serialize Deno snapshot: {err}"),
            )
        })
    }

    fn restore_session(
        &mut self,
        snapshot: &[u8],
        session_id: Option<SessionId>,
    ) -> Result<SessionId, ErrorObject> {
        let snapshot: SnapshotEnvelope = serde_json::from_slice(snapshot).map_err(|err| {
            ErrorObject::new("SNAPSHOT_CORRUPT", format!("Invalid Deno snapshot: {err}"))
        })?;
        if snapshot.magic != DENO_SNAPSHOT_MAGIC {
            return Err(ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                "Deno snapshot magic mismatch.",
            ));
        }
        if snapshot.version != langshell_core::SNAPSHOT_VERSION {
            return Err(ErrorObject::new(
                "SNAPSHOT_VERSION_MISMATCH",
                format!("Snapshot version {} is not supported.", snapshot.version),
            ));
        }
        if snapshot.capability_digest != capability_digest(&self.registry) {
            return Err(ErrorObject::new(
                "SNAPSHOT_CAPABILITY_MISMATCH",
                "Snapshot was created with a different capability set.",
            ));
        }

        let id = session_id.unwrap_or(SessionId(snapshot.session_id));
        let mut session = DenoSession::new(id.clone(), snapshot.limits, &self.registry)?;
        session.restore_globals(snapshot.globals)?;
        self.sessions.insert(id.0.clone(), session);
        Ok(id)
    }
}

struct DenoSession {
    id: SessionId,
    limits: SessionLimits,
    runtime: JsRuntime,
}

impl DenoSession {
    fn new(
        id: SessionId,
        limits: SessionLimits,
        registry: &ToolRegistry,
    ) -> Result<Self, ErrorObject> {
        let create_params = v8::Isolate::create_params().heap_limits(
            0,
            usize::try_from(limits.memory_mb).unwrap_or(usize::MAX / 1024 / 1024) * 1024 * 1024,
        );
        let mut runtime = JsRuntime::try_new(RuntimeOptions {
            extensions: vec![langshell_extension(registry.clone(), limits.clone())],
            create_params: Some(create_params),
            ..Default::default()
        })
        .map_err(|err| ErrorObject::new("RUNTIME_ERROR", err.to_string()))?;

        install_tool_globals(&mut runtime, registry)?;
        Ok(Self {
            id,
            limits,
            runtime,
        })
    }

    async fn run(&mut self, request: RunRequest, registry: &ToolRegistry) -> RunResult {
        let started = Instant::now();
        if let Some(error) = static_validation_error(&request.code, registry) {
            return RunResult::error(
                code_to_status(&error.code, true),
                error,
                String::new(),
                metrics(started, 0),
            );
        }

        let js = match transpile_typescript(&request.code) {
            Ok(js) => js,
            Err(error) => {
                return RunResult::error(
                    code_to_status(&error.code, true),
                    error,
                    String::new(),
                    metrics(started, 0),
                );
            }
        };

        self.reset_run_state(registry);
        if let Err(error) = self.inject_inputs(&request.inputs) {
            return RunResult::error(
                RunStatus::ValidationError,
                error,
                String::new(),
                metrics(started, 0),
            );
        }

        let wrapped = wrap_user_code(&js);
        let run_error = self
            .execute_user_code(
                &wrapped,
                Duration::from_millis(u64::from(request.timeout_ms.unwrap_or(self.limits.wall_ms))),
            )
            .await
            .err();
        let state = self.run_state_snapshot();
        if let Some(error) = run_error.or(state.error.clone()) {
            let mut result = RunResult::error(
                code_to_status(&error.code, false),
                error,
                truncate_stdout(state.stdout, &self.limits),
                metrics(started, state.records.len() as u32),
            );
            result.stderr = truncate_stdout(state.stderr, &self.limits);
            result.external_calls = state.records;
            return result;
        }

        let result_value = match self.read_result() {
            Ok(value) => value,
            Err(error) => {
                let mut result = RunResult::error(
                    RunStatus::ValidationError,
                    error,
                    truncate_stdout(state.stdout, &self.limits),
                    metrics(started, state.records.len() as u32),
                );
                result.stderr = truncate_stdout(state.stderr, &self.limits);
                result.external_calls = state.records;
                return result;
            }
        };

        let mut result = RunResult::ok(
            result_value,
            truncate_stdout(state.stdout, &self.limits),
            metrics(started, state.records.len() as u32),
        );
        result.stderr = truncate_stdout(state.stderr, &self.limits);
        result.external_calls = state.records;
        if request.return_snapshot {
            result.snapshot_id = Some(format!("snap_{}", digest_bytes(self.id.0.as_bytes())));
        }
        result
    }

    fn reset_run_state(&mut self, registry: &ToolRegistry) {
        let op_state = self.runtime.op_state();
        let mut state = op_state.borrow_mut();
        let data = state.borrow_mut::<DenoOpState>();
        data.registry = registry.clone();
        data.limits = self.limits.clone();
        data.stdout.clear();
        data.stderr.clear();
        data.records.clear();
        data.started_calls = 0;
        data.error = None;
    }

    fn run_state_snapshot(&self) -> RunStateSnapshot {
        let op_state = self.runtime.op_state();
        let state = op_state.borrow();
        let data = state.borrow::<DenoOpState>();
        RunStateSnapshot {
            stdout: data.stdout.clone(),
            stderr: data.stderr.clone(),
            records: data.records.clone(),
            error: data.error.clone(),
        }
    }

    fn inject_inputs(&mut self, inputs: &Map<String, Value>) -> Result<(), ErrorObject> {
        if inputs.is_empty() {
            return Ok(());
        }
        let inputs = serde_json::to_string(inputs).map_err(|err| {
            ErrorObject::new("INVALID_ARGUMENT", format!("inputs are not JSON: {err}"))
        })?;
        execute_script_unit(
            &mut self.runtime,
            "<langshell-inputs>",
            format!("globalThis.__langshell_restore_globals({inputs});"),
        )
    }

    async fn execute_user_code(
        &mut self,
        code: &str,
        timeout: Duration,
    ) -> Result<(), ErrorObject> {
        let timed_out = Arc::new(AtomicBool::new(false));
        let timer_done = Arc::new((StdMutex::new(false), Condvar::new()));
        let handle = self.runtime.v8_isolate().thread_safe_handle();
        let timer_timed_out = timed_out.clone();
        let timer_done_thread = timer_done.clone();
        let timer = std::thread::spawn(move || {
            let (lock, cvar) = &*timer_done_thread;
            let done = lock.lock().expect("timer mutex poisoned");
            let (done, wait) = cvar
                .wait_timeout_while(done, timeout, |done| !*done)
                .expect("timer condvar poisoned");
            if !*done && wait.timed_out() {
                timer_timed_out.store(true, Ordering::SeqCst);
                handle.terminate_execution();
            }
        });

        let value = match self
            .runtime
            .execute_script("<langshell-run>", code.to_owned())
        {
            Ok(value) => value,
            Err(err) => {
                stop_timer(timer_done, timer);
                if timed_out.load(Ordering::SeqCst) {
                    self.runtime.v8_isolate().cancel_terminate_execution();
                    return Err(timeout_error());
                }
                return Err(error_from_js(err.to_string(), false));
            }
        };

        let resolve = self.runtime.resolve(value);
        let resolved = tokio::time::timeout(
            timeout,
            self.runtime
                .with_event_loop_promise(resolve, PollEventLoopOptions::default()),
        )
        .await;
        stop_timer(timer_done, timer);
        if timed_out.load(Ordering::SeqCst) {
            self.runtime.v8_isolate().cancel_terminate_execution();
            return Err(timeout_error());
        }
        match resolved {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => Err(error_from_js(err.to_string(), false)),
            Err(_) => {
                self.runtime.v8_isolate().terminate_execution();
                self.runtime.v8_isolate().cancel_terminate_execution();
                Err(timeout_error())
            }
        }
    }

    fn read_result(&mut self) -> Result<Option<Value>, ErrorObject> {
        let value = self
            .runtime
            .execute_script("<langshell-result>", "globalThis.result")
            .map_err(|err| error_from_js(err.to_string(), false))?;
        v8_to_json(&mut self.runtime, value)
    }

    fn snapshot_globals(&mut self) -> Result<Value, ErrorObject> {
        let value = self
            .runtime
            .execute_script(
                "<langshell-snapshot>",
                "JSON.stringify(globalThis.__langshell_snapshot_globals())",
            )
            .map_err(|err| ErrorObject::new("SNAPSHOT_CORRUPT", err.to_string()))?;
        let globals = v8_to_string(&mut self.runtime, value)?;
        serde_json::from_str(&globals).map_err(|err| {
            ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                format!("Deno snapshot globals did not produce valid JSON: {err}"),
            )
        })
    }

    fn restore_globals(&mut self, globals: Value) -> Result<(), ErrorObject> {
        let globals = serde_json::to_string(&globals).map_err(|err| {
            ErrorObject::new("SNAPSHOT_CORRUPT", format!("Invalid globals: {err}"))
        })?;
        execute_script_unit(
            &mut self.runtime,
            "<langshell-restore>",
            format!("globalThis.__langshell_restore_globals({globals});"),
        )
        .map_err(|err| ErrorObject::new("SNAPSHOT_CORRUPT", err.message))
    }
}

fn stop_timer(timer_done: Arc<(StdMutex<bool>, Condvar)>, timer: std::thread::JoinHandle<()>) {
    let (lock, cvar) = &*timer_done;
    if let Ok(mut done) = lock.lock() {
        *done = true;
        cvar.notify_one();
    }
    let _ = timer.join();
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotEnvelope {
    magic: String,
    version: u32,
    session_id: String,
    limits: SessionLimits,
    globals: Value,
    capability_digest: String,
}

struct DenoOpState {
    registry: ToolRegistry,
    limits: SessionLimits,
    stdout: String,
    stderr: String,
    records: Vec<ExternalCallRecord>,
    started_calls: u32,
    error: Option<ErrorObject>,
}

impl DenoOpState {
    fn new(registry: ToolRegistry, limits: SessionLimits) -> Self {
        Self {
            registry,
            limits,
            stdout: String::new(),
            stderr: String::new(),
            records: Vec::new(),
            started_calls: 0,
            error: None,
        }
    }
}

struct RunStateSnapshot {
    stdout: String,
    stderr: String,
    records: Vec<ExternalCallRecord>,
    error: Option<ErrorObject>,
}

#[op2(fast)]
pub fn op_print(state: &mut OpState, #[string] msg: &str, is_err: bool) {
    let data = state.borrow_mut::<DenoOpState>();
    if is_err {
        data.stderr.push_str(msg);
    } else {
        data.stdout.push_str(msg);
    }
}

#[op2]
#[serde]
fn op_langshell_call_tool_sync(
    state: &mut OpState,
    #[string] name: String,
    #[serde] args: Vec<serde_json::Value>,
    #[serde] kwargs: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, JsErrorBox> {
    let (tool, ctx) = prepare_tool_call(state, name, args, kwargs)?;
    if tool.async_mode {
        let error = ErrorObject::new(
            "TYPE_ERROR",
            format!(
                "Tool {} is asynchronous; call it with await.",
                tool.capability.name
            ),
        );
        state.borrow_mut::<DenoOpState>().error = Some(error.clone());
        return Err(error_object_to_js(error));
    }
    let outcome = futures::executor::block_on(run_tool(tool, ctx));
    finish_tool_call(state, outcome)
}

#[op2(async(lazy))]
#[serde]
async fn op_langshell_call_tool_async(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] args: Vec<serde_json::Value>,
    #[serde] kwargs: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, JsErrorBox> {
    let (tool, ctx) = {
        let mut state = state.borrow_mut();
        prepare_tool_call(&mut state, name, args, kwargs)?
    };
    let outcome = run_tool(tool, ctx).await;
    let mut state = state.borrow_mut();
    finish_tool_call(&mut state, outcome)
}

fn langshell_extension(registry: ToolRegistry, limits: SessionLimits) -> Extension {
    Extension {
        name: "langshell_deno",
        ops: std::borrow::Cow::Owned(vec![
            op_langshell_call_tool_sync(),
            op_langshell_call_tool_async(),
        ]),
        middleware_fn: Some(Box::new(|op| match op.name {
            "op_print" => op_print(),
            _ => op,
        })),
        op_state_fn: Some(Box::new(move |state| {
            state.put(DenoOpState::new(registry, limits));
        })),
        ..Default::default()
    }
}

fn install_tool_globals(
    runtime: &mut JsRuntime,
    registry: &ToolRegistry,
) -> Result<(), ErrorObject> {
    let tools: Vec<_> = registry
        .names()
        .into_iter()
        .filter_map(|name| {
            registry.get(&name).map(|tool| {
                json!({
                    "name": name,
                    "asyncMode": tool.async_mode,
                })
            })
        })
        .collect();
    let tools = serde_json::to_string(&tools)
        .map_err(|err| ErrorObject::new("SERIALIZE_ERROR", format!("tool metadata: {err}")))?;
    let script = format!(
        r#"
const __langshellToolDefs = {tools};
const __langshellOps = Deno.core.ops;
Object.defineProperty(globalThis, "__langshell_ops", {{ value: __langshellOps, configurable: false }});
Object.defineProperty(globalThis, "LangShell", {{
  value: Object.freeze({{
    callTool: (name, args = [], kwargs = {{}}) => __langshellOps.op_langshell_call_tool_async(name, args, kwargs),
    callToolSync: (name, args = [], kwargs = {{}}) => __langshellOps.op_langshell_call_tool_sync(name, args, kwargs),
  }}),
  configurable: true,
}});
for (const tool of __langshellToolDefs) {{
  const call = tool.asyncMode
    ? (...args) => __langshellOps.op_langshell_call_tool_async(tool.name, args, {{}})
    : (...args) => __langshellOps.op_langshell_call_tool_sync(tool.name, args, {{}});
  Object.defineProperty(globalThis, tool.name, {{ value: call, writable: false, configurable: true }});
}}
Object.defineProperty(globalThis, "__langshell_restore_globals", {{
  value: (globals) => {{
    for (const [key, value] of Object.entries(globals ?? {{}})) {{
      Object.defineProperty(globalThis, key, {{ value, writable: true, configurable: true }});
    }}
  }},
  configurable: false,
}});
Object.defineProperty(globalThis, "__langshell_snapshot_globals", {{
  value: () => {{
    const out = {{}};
    for (const key of Object.getOwnPropertyNames(globalThis)) {{
      if (globalThis.__langshell_baseline.has(key) || key.startsWith("__langshell")) continue;
      const value = globalThis[key];
      if (typeof value === "function" || typeof value === "symbol" || typeof value === "undefined" || typeof value === "bigint") continue;
      try {{
        JSON.stringify(value);
        out[key] = value;
      }} catch (_) {{}}
    }}
    return out;
  }},
  configurable: false,
}});
Object.defineProperty(globalThis, "__langshell_baseline", {{
  value: new Set(Object.getOwnPropertyNames(globalThis)),
  configurable: false,
}});
try {{
  Object.defineProperty(globalThis, "Deno", {{ value: undefined, writable: false, configurable: true }});
}} catch (_) {{}}
"#
    );
    execute_script_unit(runtime, "<langshell-bootstrap>", script)
}

fn prepare_tool_call(
    state: &mut OpState,
    name: String,
    args: Vec<Value>,
    kwargs: Map<String, Value>,
) -> Result<(langshell_core::RegisteredTool, ToolCallContext), JsErrorBox> {
    let data = state.borrow_mut::<DenoOpState>();
    data.started_calls = data.started_calls.saturating_add(1);
    if data.started_calls > data.limits.max_external_calls {
        let error = ErrorObject::new(
            "EXTERNAL_CALLS_EXCEEDED",
            format!(
                "External call limit {} exceeded.",
                data.limits.max_external_calls
            ),
        );
        data.error = Some(error.clone());
        return Err(error_object_to_js(error));
    }
    let Some(tool) = data.registry.get(&name).cloned() else {
        let error = ErrorObject::new(
            "UNKNOWN_TOOL",
            format!("Function {name} is not registered."),
        )
        .with_hint("Call list_tools() or describe_tool() to inspect registered functions.");
        data.error = Some(error.clone());
        return Err(error_object_to_js(error));
    };
    Ok((tool, ToolCallContext { name, args, kwargs }))
}

async fn run_tool(
    tool: langshell_core::RegisteredTool,
    ctx: ToolCallContext,
) -> (Result<Value, ErrorObject>, ExternalCallRecord) {
    let started = Instant::now();
    let request_digest = digest_json(&json!({"args": ctx.args, "kwargs": ctx.kwargs}));
    let side_effect = tool.capability.side_effect;
    let name = tool.capability.name.clone();
    match tool.call(ctx).await {
        Ok(value) => {
            let response_digest = Some(digest_json(&value));
            (
                Ok(value),
                ExternalCallRecord {
                    name,
                    side_effect,
                    duration_ms: elapsed_ms(started),
                    status: CallStatus::Ok,
                    request_digest,
                    response_digest,
                    error: None,
                },
            )
        }
        Err(error) => {
            let error_object = ErrorObject::new(error.code, error.message);
            (
                Err(error_object.clone()),
                ExternalCallRecord {
                    name,
                    side_effect,
                    duration_ms: elapsed_ms(started),
                    status: CallStatus::Error,
                    request_digest,
                    response_digest: None,
                    error: Some(error_object),
                },
            )
        }
    }
}

fn finish_tool_call(
    state: &mut OpState,
    outcome: (Result<Value, ErrorObject>, ExternalCallRecord),
) -> Result<Value, JsErrorBox> {
    let (result, record) = outcome;
    let data = state.borrow_mut::<DenoOpState>();
    data.records.push(record);
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            data.error = Some(error.clone());
            Err(error_object_to_js(error))
        }
    }
}

fn validate_request(request: &RunRequest, registry: &ToolRegistry) -> RunResult {
    let started = Instant::now();
    if let Some(error) = static_validation_error(&request.code, registry) {
        return RunResult::error(
            code_to_status(&error.code, true),
            error,
            String::new(),
            metrics(started, 0),
        );
    }
    match transpile_typescript(&request.code) {
        Ok(_) => RunResult::ok(None, String::new(), metrics(started, 0)),
        Err(error) => RunResult::error(
            code_to_status(&error.code, true),
            error,
            String::new(),
            metrics(started, 0),
        ),
    }
}

fn transpile_typescript(code: &str) -> Result<String, ErrorObject> {
    let specifier = deno_core::resolve_url("file:///langshell-run.ts")
        .map_err(|err| ErrorObject::new("RUNTIME_ERROR", err.to_string()))?;
    let parsed = deno_ast::parse_module(ParseParams {
        specifier,
        text: code.to_owned().into(),
        media_type: MediaType::TypeScript,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|err| ErrorObject::new("SYNTAX_ERROR", err.to_string()))?;
    let transpiled = parsed
        .transpile(
            &deno_ast::TranspileOptions {
                imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
                decorators: deno_ast::DecoratorsTranspileOption::Ecma,
                ..Default::default()
            },
            &deno_ast::TranspileModuleOptions { module_kind: None },
            &deno_ast::EmitOptions {
                source_map: SourceMapOption::None,
                inline_sources: false,
                ..Default::default()
            },
        )
        .map_err(|err| ErrorObject::new("SYNTAX_ERROR", err.to_string()))?;
    Ok(transpiled.into_source().text)
}

fn wrap_user_code(js: &str) -> String {
    format!(
        r#"
(async () => {{
  with (globalThis) {{
{js}
  }}
}})()
"#
    )
}

fn v8_to_json(
    runtime: &mut JsRuntime,
    value: v8::Global<v8::Value>,
) -> Result<Option<Value>, ErrorObject> {
    deno_core::scope!(scope, runtime);
    let local = v8::Local::new(scope, value);
    if local.is_undefined() {
        return Ok(None);
    }
    serde_v8::from_v8::<Value>(scope, local)
        .map(Some)
        .map_err(|err| {
            ErrorObject::new(
                "RESULT_NOT_SERIALIZABLE",
                format!("TypeScript result could not be converted to JSON: {err}"),
            )
            .with_hint("Assign result to a JSON-compatible value before returning.")
        })
}

fn v8_to_string(
    runtime: &mut JsRuntime,
    value: v8::Global<v8::Value>,
) -> Result<String, ErrorObject> {
    deno_core::scope!(scope, runtime);
    let local = v8::Local::new(scope, value);
    serde_v8::from_v8::<String>(scope, local).map_err(|err| {
        ErrorObject::new(
            "RESULT_NOT_SERIALIZABLE",
            format!("TypeScript value could not be converted to string: {err}"),
        )
    })
}

fn execute_script_unit(
    runtime: &mut JsRuntime,
    name: &'static str,
    source: String,
) -> Result<(), ErrorObject> {
    runtime
        .execute_script(name, source)
        .map(|_| ())
        .map_err(|err| error_from_js(err.to_string(), false))
}

fn static_validation_error(code: &str, registry: &ToolRegistry) -> Option<ErrorObject> {
    let unsupported = [
        "import ",
        "export ",
        "import(",
        "require(",
        "Deno.",
        "Deno[",
        "fetch(",
        "XMLHttpRequest",
        "WebSocket",
        "process.",
        "Bun.",
        "eval(",
        "Function(",
    ];
    unsupported
        .iter()
        .find(|pattern| code.contains(**pattern))
        .map(|pattern| {
            ErrorObject::new(
                "UNSUPPORTED_FEATURE",
                format!("Use of {pattern:?} is not supported in the LangShell Deno sandbox."),
            )
            .with_hint(
                "Use a registered capability such as read_text, fetch_json, or list_tools instead.",
            )
        })
        .or_else(|| {
            let suspicious = ["fetch_url", "query_db", "send_email"];
            suspicious
                .iter()
                .find(|name| code.contains(&format!("{name}(")) && !registry.contains(name))
                .map(|name| {
                    ErrorObject::new(
                        "UNKNOWN_TOOL",
                        format!("Function {name} is not registered."),
                    )
                    .with_hint("Call list_tools() to inspect available capabilities.")
                })
        })
}

fn effective_limits(default_limits: &SessionLimits, request: &RunRequest) -> SessionLimits {
    let mut limits = request
        .limits
        .clone()
        .unwrap_or_else(|| default_limits.clone());
    if let Some(timeout_ms) = request.timeout_ms {
        limits.wall_ms = timeout_ms;
    }
    limits
}

fn code_to_status(code: &str, validation: bool) -> RunStatus {
    match code {
        "PERMISSION_DENIED" => RunStatus::PermissionDenied,
        "WAITING_FOR_APPROVAL" => RunStatus::WaitingForApproval,
        "TIMEOUT_WALL" | "TIMEOUT_CPU" | "TIMEOUT_TOOL" => RunStatus::Timeout,
        "CANCELLED" => RunStatus::Cancelled,
        "MEMORY_EXCEEDED" | "STDOUT_EXCEEDED" | "EXTERNAL_CALLS_EXCEEDED" | "STACK_OVERFLOW" => {
            RunStatus::ResourceExhausted
        }
        "SYNTAX_ERROR"
        | "TYPE_ERROR"
        | "UNKNOWN_TOOL"
        | "UNSUPPORTED_FEATURE"
        | "RESULT_NOT_SERIALIZABLE"
        | "SNAPSHOT_VERSION_MISMATCH"
        | "SNAPSHOT_CAPABILITY_MISMATCH"
        | "SNAPSHOT_CORRUPT" => RunStatus::ValidationError,
        _ if validation => RunStatus::ValidationError,
        _ => RunStatus::RuntimeError,
    }
}

fn error_from_js(message: String, validation: bool) -> ErrorObject {
    let code = if validation || message.contains("SyntaxError") {
        "SYNTAX_ERROR"
    } else if message.contains("execution terminated") {
        "TIMEOUT_WALL"
    } else {
        "RUNTIME_ERROR"
    };
    ErrorObject::new(code, message)
}

fn error_object_to_js(error: ErrorObject) -> JsErrorBox {
    JsErrorBox::generic(format!("{}: {}", error.code, error.message))
}

fn timeout_error() -> ErrorObject {
    ErrorObject::new(
        "TIMEOUT_WALL",
        "TypeScript execution exceeded the wall-clock limit.",
    )
}

fn runtime_error_result(error: ErrorObject) -> RunResult {
    RunResult::error(
        RunStatus::RuntimeError,
        error,
        String::new(),
        Metrics::default(),
    )
}

fn worker_closed_error() -> ErrorObject {
    ErrorObject::new("RUNTIME_ERROR", "LangShell Deno worker is not available.")
}

fn truncate_stdout(mut stdout: String, limits: &SessionLimits) -> String {
    let max = limits.max_stdout_bytes as usize;
    if stdout.len() > max {
        stdout.truncate(max);
    }
    stdout
}

fn metrics(started: Instant, external_calls_count: u32) -> Metrics {
    Metrics {
        duration_ms: elapsed_ms(started),
        memory_peak_bytes: 0,
        instructions: 0,
        external_calls_count,
    }
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

fn capability_digest(registry: &ToolRegistry) -> String {
    digest_json(&json!(registry.names()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use langshell_core::{Capability, RegisteredTool, SideEffect};

    #[tokio::test]
    async fn runs_typescript_and_reuses_state() {
        let runtime = DenoRuntime::new(ToolRegistry::new(), SessionLimits::default());
        let mut first = RunRequest::new("s1", "cache = { k: 1 }").unwrap();
        first.language = Language::TypeScript;
        assert_eq!(runtime.run(first).await.status, RunStatus::Ok);

        let mut second = RunRequest::new("s1", "result = cache.k + 1").unwrap();
        second.language = Language::TypeScript;
        let result = runtime.run(second).await;
        assert_eq!(result.status, RunStatus::Ok, "{result:?}");
        assert_eq!(result.result, Some(json!(2)));
    }

    #[tokio::test]
    async fn supports_async_external_function() {
        let mut registry = ToolRegistry::new();
        registry
            .register(RegisteredTool::asynchronous(
                Capability::new("fetch_json", "test fetch", SideEffect::Network),
                |ctx| {
                    Box::pin(async move {
                        Ok(json!({
                            "url": ctx.args.first().and_then(Value::as_str).unwrap_or_default(),
                        }))
                    })
                },
            ))
            .unwrap();
        let runtime = DenoRuntime::new(registry, SessionLimits::default());
        let mut request = RunRequest::new(
            "s1",
            r#"
data = await fetch_json("https://api.example.com/item")
result = { url: data.url }
"#,
        )
        .unwrap();
        request.language = Language::TypeScript;
        let result = runtime.run(request).await;
        assert_eq!(result.status, RunStatus::Ok, "{result:?}");
        assert_eq!(
            result.result,
            Some(json!({"url": "https://api.example.com/item"}))
        );
        assert_eq!(result.metrics.external_calls_count, 1);
    }

    #[tokio::test]
    async fn snapshots_json_globals() {
        let runtime = DenoRuntime::new(ToolRegistry::new(), SessionLimits::default());
        let mut first = RunRequest::new("s1", "state = { step: 1 }").unwrap();
        first.language = Language::TypeScript;
        assert_eq!(runtime.run(first).await.status, RunStatus::Ok);

        let snapshot = runtime
            .snapshot_session(&SessionId("s1".to_owned()))
            .await
            .unwrap();
        runtime
            .restore_session(&snapshot, Some(SessionId("s2".to_owned())))
            .await
            .unwrap();

        let mut second = RunRequest::new("s2", "result = state.step").unwrap();
        second.language = Language::TypeScript;
        let result = runtime.run(second).await;
        assert_eq!(result.status, RunStatus::Ok, "{result:?}");
        assert_eq!(result.result, Some(json!(1)));
    }
}
