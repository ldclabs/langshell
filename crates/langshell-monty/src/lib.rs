use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use langshell_core::{
    CallStatus, ErrorObject, ExternalCallRecord, Language, Metrics, RunRequest, RunResult,
    RunStatus, SessionId, SessionLimits, ToolCallContext, ToolRegistry, digest_bytes, digest_json,
};
use monty::{
    ExcType, ExtFunctionResult, JsonMontyObject, LimitedTracker, MontyException, MontyObject,
    MontyRepl, NameLookupResult, PrintWriter, ReplProgress, ResourceLimits,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct MontyRuntime {
    sessions: Mutex<HashMap<String, MontySession>>,
    registry: ToolRegistry,
    default_limits: SessionLimits,
}

impl MontyRuntime {
    pub fn new(registry: ToolRegistry, default_limits: SessionLimits) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            registry,
            default_limits,
        }
    }

    pub async fn create_session(&self, session_id: SessionId, limits: Option<SessionLimits>) {
        let limits = limits.unwrap_or_else(|| self.default_limits.clone());
        let mut sessions = self.sessions.lock().await;
        sessions
            .entry(session_id.0.clone())
            .or_insert_with(|| MontySession::new(session_id, limits));
    }

    pub async fn run(&self, request: RunRequest) -> RunResult {
        if request.language != Language::Python {
            return RunResult::error(
                RunStatus::ValidationError,
                ErrorObject::new(
                    "UNSUPPORTED_FEATURE",
                    "Only the Python backend is available in the MVP.",
                ),
                String::new(),
                Metrics::default(),
            );
        }

        if request.validate_only {
            return self.validate(&request);
        }

        let limits = effective_limits(&self.default_limits, &request);
        let session_id = request.session_id.clone();
        let mut session = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .remove(&session_id.0)
                .unwrap_or_else(|| MontySession::new(session_id.clone(), limits.clone()))
        };
        session.limits = limits;

        let registry = self.registry.clone();
        let (session, result) = run_session(session, request, registry).await;
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id.0, session);
        result
    }

    pub fn validate(&self, request: &RunRequest) -> RunResult {
        let started = Instant::now();
        if let Some(error) = static_validation_error(&request.code, &self.registry) {
            return RunResult::error(
                code_to_status(&error.code, true),
                error,
                String::new(),
                metrics(started, 0, 0),
            );
        }

        match monty::MontyRun::new(
            request.code.clone(),
            "<validate>",
            request.inputs.keys().cloned().collect(),
        ) {
            Ok(_) => RunResult::ok(None, String::new(), metrics(started, 0, 0)),
            Err(error) => {
                let error = error_from_exception(error, true);
                RunResult::error(
                    code_to_status(&error.code, true),
                    error,
                    String::new(),
                    metrics(started, 0, 0),
                )
            }
        }
    }

    pub async fn destroy_session(&self, session_id: &SessionId) -> bool {
        self.sessions.lock().await.remove(&session_id.0).is_some()
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        let mut ids: Vec<_> = self
            .sessions
            .lock()
            .await
            .keys()
            .cloned()
            .map(SessionId)
            .collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        ids
    }

    pub async fn snapshot_session(&self, session_id: &SessionId) -> Result<Vec<u8>, ErrorObject> {
        let sessions = self.sessions.lock().await;
        let session = sessions.get(&session_id.0).ok_or_else(|| {
            ErrorObject::new(
                "SESSION_NOT_FOUND",
                format!("Session {} does not exist.", session_id.0),
            )
        })?;
        let repl_dump = session.repl.dump().map_err(|err| {
            ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                format!("Failed to encode snapshot: {err}"),
            )
        })?;
        let snapshot = SnapshotEnvelope {
            magic: SNAPSHOT_MAGIC.to_owned(),
            version: langshell_core::SNAPSHOT_VERSION,
            session_id: session_id.0.clone(),
            limits: session.limits.clone(),
            repl_dump,
            capability_digest: capability_digest(&self.registry),
        };
        serde_json::to_vec(&snapshot).map_err(|err| {
            ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                format!("Failed to serialize snapshot: {err}"),
            )
        })
    }

    pub async fn restore_session(
        &self,
        snapshot: &[u8],
        session_id: Option<SessionId>,
    ) -> Result<SessionId, ErrorObject> {
        let snapshot: SnapshotEnvelope = serde_json::from_slice(snapshot).map_err(|err| {
            ErrorObject::new("SNAPSHOT_CORRUPT", format!("Invalid snapshot: {err}"))
        })?;
        if snapshot.magic != SNAPSHOT_MAGIC {
            return Err(ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                "Snapshot magic mismatch.",
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
        let repl = MontyRepl::load(&snapshot.repl_dump).map_err(|err| {
            ErrorObject::new(
                "SNAPSHOT_CORRUPT",
                format!("Failed to decode Monty state: {err}"),
            )
        })?;
        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            id.0.clone(),
            MontySession {
                id: id.clone(),
                limits: snapshot.limits,
                repl,
            },
        );
        Ok(id)
    }
}

#[derive(Debug)]
struct MontySession {
    id: SessionId,
    limits: SessionLimits,
    repl: MontyRepl<LimitedTracker>,
}

impl MontySession {
    fn new(id: SessionId, limits: SessionLimits) -> Self {
        let tracker = LimitedTracker::new(resource_limits(&limits));
        Self {
            id,
            limits,
            repl: MontyRepl::new("<langshell-session>", tracker),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotEnvelope {
    magic: String,
    version: u32,
    session_id: String,
    limits: SessionLimits,
    repl_dump: Vec<u8>,
    capability_digest: String,
}

const SNAPSHOT_MAGIC: &str = "langshell-snapshot/v1";

async fn run_session(
    mut session: MontySession,
    request: RunRequest,
    registry: ToolRegistry,
) -> (MontySession, RunResult) {
    let started = Instant::now();
    if let Some(error) = static_validation_error(&request.code, &registry) {
        let result = RunResult::error(
            code_to_status(&error.code, true),
            error,
            String::new(),
            metrics(started, session.repl.tracker().current_memory() as u64, 0),
        );
        return (session, result);
    }

    session
        .repl
        .tracker_mut()
        .set_max_duration(Duration::from_millis(u64::from(
            request.timeout_ms.unwrap_or(session.limits.wall_ms),
        )));

    let inputs = match request
        .inputs
        .iter()
        .map(|(name, value)| json_to_monty(value.clone()).map(|value| (name.clone(), value)))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(inputs) => inputs,
        Err(error) => {
            let result = RunResult::error(
                RunStatus::ValidationError,
                error,
                String::new(),
                metrics(started, session.repl.tracker().current_memory() as u64, 0),
            );
            return (session, result);
        }
    };

    let repl = session.repl;
    let run_outcome =
        run_repl_snippet(repl, &request.code, inputs, &registry, &session.limits).await;
    match run_outcome {
        SnippetOutcome::Complete {
            repl,
            value,
            stdout,
            mut records,
        } => {
            let expression_result = if matches!(value, MontyObject::None) {
                None
            } else {
                match monty_to_json_checked(&value) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        session.repl = repl;
                        let mut result = RunResult::error(
                            RunStatus::ValidationError,
                            error,
                            stdout,
                            metrics(
                                started,
                                session.repl.tracker().current_memory() as u64,
                                records.len() as u32,
                            ),
                        );
                        result.external_calls = records;
                        return (session, result);
                    }
                }
            };

            let probe =
                run_repl_snippet(repl, "result", Vec::new(), &registry, &session.limits).await;
            let (next_repl, result_value, stdout) = match probe {
                SnippetOutcome::Complete {
                    repl,
                    value,
                    stdout: probe_stdout,
                    records: probe_records,
                } => {
                    records.extend(probe_records);
                    match monty_to_json_checked(&value) {
                        Ok(value) => (repl, Some(value), stdout + &probe_stdout),
                        Err(error) => {
                            session.repl = repl;
                            let mut result = RunResult::error(
                                RunStatus::ValidationError,
                                error,
                                stdout + &probe_stdout,
                                metrics(
                                    started,
                                    session.repl.tracker().current_memory() as u64,
                                    records.len() as u32,
                                ),
                            );
                            result.external_calls = records;
                            return (session, result);
                        }
                    }
                }
                SnippetOutcome::Error {
                    repl,
                    error,
                    stdout: probe_stdout,
                    records: probe_records,
                } if error.code == "UNKNOWN_TOOL" || error.code == "RUNTIME_ERROR" => {
                    records.extend(probe_records);
                    (repl, expression_result, stdout + &probe_stdout)
                }
                SnippetOutcome::Error {
                    repl,
                    error,
                    stdout: probe_stdout,
                    records: probe_records,
                } => {
                    records.extend(probe_records);
                    session.repl = repl;
                    let mut result = RunResult::error(
                        code_to_status(&error.code, false),
                        error,
                        stdout + &probe_stdout,
                        metrics(
                            started,
                            session.repl.tracker().current_memory() as u64,
                            records.len() as u32,
                        ),
                    );
                    result.external_calls = records;
                    return (session, result);
                }
            };

            session.repl = next_repl;
            let mut result = RunResult::ok(
                result_value,
                truncate_stdout(stdout, &session.limits),
                metrics(
                    started,
                    session.repl.tracker().current_memory() as u64,
                    records.len() as u32,
                ),
            );
            result.external_calls = records;
            if request.return_snapshot {
                result.snapshot_id =
                    Some(format!("snap_{}", digest_bytes(session.id.0.as_bytes())));
            }
            (session, result)
        }
        SnippetOutcome::Error {
            repl,
            error,
            stdout,
            records,
        } => {
            session.repl = repl;
            let mut result = RunResult::error(
                code_to_status(&error.code, false),
                error,
                truncate_stdout(stdout, &session.limits),
                metrics(
                    started,
                    session.repl.tracker().current_memory() as u64,
                    records.len() as u32,
                ),
            );
            result.external_calls = records;
            (session, result)
        }
    }
}

enum SnippetOutcome {
    Complete {
        repl: MontyRepl<LimitedTracker>,
        value: MontyObject,
        stdout: String,
        records: Vec<ExternalCallRecord>,
    },
    Error {
        repl: MontyRepl<LimitedTracker>,
        error: ErrorObject,
        stdout: String,
        records: Vec<ExternalCallRecord>,
    },
}

type PendingTool = BoxFuture<'static, (u32, ExtFunctionResult, ExternalCallRecord)>;

async fn run_repl_snippet(
    repl: MontyRepl<LimitedTracker>,
    code: &str,
    inputs: Vec<(String, MontyObject)>,
    registry: &ToolRegistry,
    limits: &SessionLimits,
) -> SnippetOutcome {
    let mut stdout = String::new();
    let mut records = Vec::new();
    let mut started_calls = 0u32;
    let mut pending = FuturesUnordered::<PendingTool>::new();
    let mut ready = Vec::<(u32, ExtFunctionResult, ExternalCallRecord)>::new();

    let mut progress = match repl.feed_start(code, inputs, PrintWriter::CollectString(&mut stdout))
    {
        Ok(progress) => progress,
        Err(error) => {
            let error = *error;
            return SnippetOutcome::Error {
                repl: error.repl,
                error: error_from_exception(error.error, false),
                stdout,
                records,
            };
        }
    };

    loop {
        match progress {
            ReplProgress::Complete { repl, value } => {
                return SnippetOutcome::Complete {
                    repl,
                    value,
                    stdout,
                    records,
                };
            }
            ReplProgress::NameLookup(lookup) => {
                let result =
                    registry
                        .get(&lookup.name)
                        .map_or(NameLookupResult::Undefined, |tool| {
                            NameLookupResult::Value(MontyObject::Function {
                                name: tool.capability.name.clone(),
                                docstring: Some(tool.capability.description.clone()),
                            })
                        });
                progress = match lookup.resume(result, PrintWriter::CollectString(&mut stdout)) {
                    Ok(progress) => progress,
                    Err(error) => {
                        let error = *error;
                        return SnippetOutcome::Error {
                            repl: error.repl,
                            error: error_from_exception(error.error, false),
                            stdout,
                            records,
                        };
                    }
                };
            }
            ReplProgress::FunctionCall(call) => {
                started_calls = started_calls.saturating_add(1);
                if started_calls > limits.max_external_calls {
                    return SnippetOutcome::Error {
                        repl: call.into_repl(),
                        error: ErrorObject::new(
                            "EXTERNAL_CALLS_EXCEEDED",
                            format!(
                                "External call limit exceeded: {}.",
                                limits.max_external_calls
                            ),
                        ),
                        stdout,
                        records,
                    };
                }

                let Some(tool) = registry.get(&call.function_name).cloned() else {
                    let function_name = call.function_name.clone();
                    return SnippetOutcome::Error {
                        repl: call.into_repl(),
                        error: ErrorObject::new(
                            "UNKNOWN_TOOL",
                            format!("Function {function_name} is not registered in this session."),
                        )
                        .with_hint("Call list_tools() to inspect available capabilities."),
                        stdout,
                        records,
                    };
                };

                let ctx = match tool_context(&tool.capability.name, &call.args, &call.kwargs) {
                    Ok(ctx) => ctx,
                    Err(error) => {
                        return SnippetOutcome::Error {
                            repl: call.into_repl(),
                            error,
                            stdout,
                            records,
                        };
                    }
                };

                if tool.async_mode {
                    let call_id = call.call_id;
                    pending.push(run_tool_async(call_id, tool, ctx).boxed());
                    progress = match call.resume_pending(PrintWriter::CollectString(&mut stdout)) {
                        Ok(progress) => progress,
                        Err(error) => {
                            let error = *error;
                            return SnippetOutcome::Error {
                                repl: error.repl,
                                error: error_from_exception(error.error, false),
                                stdout,
                                records,
                            };
                        }
                    };
                } else {
                    let (_call_id, result, record) = run_tool_async(call.call_id, tool, ctx).await;
                    records.push(record);
                    progress = match call.resume(result, PrintWriter::CollectString(&mut stdout)) {
                        Ok(progress) => progress,
                        Err(error) => {
                            let error = *error;
                            return SnippetOutcome::Error {
                                repl: error.repl,
                                error: error_from_exception(error.error, false),
                                stdout,
                                records,
                            };
                        }
                    };
                }
            }
            ReplProgress::ResolveFutures(state) => {
                let pending_ids = state.pending_call_ids().to_vec();
                let mut resolved = drain_ready_for(&pending_ids, &mut ready, &mut records);
                while resolved.is_empty() {
                    let Some(item) = pending.next().await else {
                        let repl = state.into_repl();
                        return SnippetOutcome::Error {
                            repl,
                            error: ErrorObject::new(
                                "RUNTIME_ERROR",
                                "Monty requested async future results but no host futures are pending.",
                            ),
                            stdout,
                            records,
                        };
                    };
                    if pending_ids.contains(&item.0) {
                        records.push(item.2);
                        resolved.push((item.0, item.1));
                    } else {
                        ready.push(item);
                    }
                }
                progress = match state.resume(resolved, PrintWriter::CollectString(&mut stdout)) {
                    Ok(progress) => progress,
                    Err(error) => {
                        let error = *error;
                        return SnippetOutcome::Error {
                            repl: error.repl,
                            error: error_from_exception(error.error, false),
                            stdout,
                            records,
                        };
                    }
                };
            }
            ReplProgress::OsCall(call) => {
                let error = call.function.on_no_handler(&call.args);
                progress = match call.resume(
                    ExtFunctionResult::Error(error),
                    PrintWriter::CollectString(&mut stdout),
                ) {
                    Ok(progress) => progress,
                    Err(error) => {
                        let error = *error;
                        return SnippetOutcome::Error {
                            repl: error.repl,
                            error: error_from_exception(error.error, false),
                            stdout,
                            records,
                        };
                    }
                };
            }
        }
    }
}

async fn run_tool_async(
    call_id: u32,
    tool: langshell_core::RegisteredTool,
    ctx: ToolCallContext,
) -> (u32, ExtFunctionResult, ExternalCallRecord) {
    let started = Instant::now();
    let request_digest = digest_json(&json!({"args": ctx.args, "kwargs": ctx.kwargs}));
    let side_effect = tool.capability.side_effect;
    let name = tool.capability.name.clone();

    match tool.call(ctx).await {
        Ok(value) => {
            let response_digest = Some(digest_json(&value));
            let result = json_to_monty(value)
                .map(ExtFunctionResult::Return)
                .unwrap_or_else(|error| {
                    ExtFunctionResult::Error(error_object_to_exception(&error))
                });
            (
                call_id,
                result,
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
                call_id,
                ExtFunctionResult::Error(error_object_to_exception(&error_object)),
                ExternalCallRecord {
                    name,
                    side_effect,
                    duration_ms: elapsed_ms(started),
                    status: if error_object.code == "PERMISSION_DENIED" {
                        CallStatus::Denied
                    } else {
                        CallStatus::Error
                    },
                    request_digest,
                    response_digest: None,
                    error: Some(error_object),
                },
            )
        }
    }
}

fn drain_ready_for(
    pending_ids: &[u32],
    ready: &mut Vec<(u32, ExtFunctionResult, ExternalCallRecord)>,
    records: &mut Vec<ExternalCallRecord>,
) -> Vec<(u32, ExtFunctionResult)> {
    let mut resolved = Vec::new();
    let mut index = 0;
    while index < ready.len() {
        if pending_ids.contains(&ready[index].0) {
            let (call_id, result, record) = ready.remove(index);
            records.push(record);
            resolved.push((call_id, result));
        } else {
            index += 1;
        }
    }
    resolved
}

fn tool_context(
    name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
) -> Result<ToolCallContext, ErrorObject> {
    let args = args
        .iter()
        .map(monty_to_json_lossy)
        .collect::<Result<Vec<_>, _>>()?;
    let mut kwargs_json = Map::new();
    for (key, value) in kwargs {
        let key = match key {
            MontyObject::String(key) => key.clone(),
            other => other.py_repr(),
        };
        kwargs_json.insert(key, monty_to_json_lossy(value)?);
    }
    Ok(ToolCallContext {
        name: name.to_owned(),
        args,
        kwargs: kwargs_json,
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

fn resource_limits(limits: &SessionLimits) -> ResourceLimits {
    ResourceLimits::new()
        .max_duration(Duration::from_millis(u64::from(limits.wall_ms)))
        .max_memory(limits.memory_mb as usize * 1024 * 1024)
        .max_recursion_depth(Some(usize::from(limits.max_stack_depth)))
}

fn json_to_monty(value: Value) -> Result<MontyObject, ErrorObject> {
    Ok(match value {
        Value::Null => MontyObject::None,
        Value::Bool(value) => MontyObject::Bool(value),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                MontyObject::Int(value)
            } else if let Some(value) = number.as_u64().and_then(|value| i64::try_from(value).ok())
            {
                MontyObject::Int(value)
            } else if let Some(value) = number.as_f64() {
                MontyObject::Float(value)
            } else {
                return Err(ErrorObject::new(
                    "TYPE_ERROR",
                    "JSON number cannot be represented in Monty.",
                ));
            }
        }
        Value::String(value) => MontyObject::String(value),
        Value::Array(items) => MontyObject::List(
            items
                .into_iter()
                .map(json_to_monty)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Object(map) => MontyObject::Dict(
            map.into_iter()
                .map(|(key, value)| {
                    json_to_monty(value).map(|value| (MontyObject::String(key), value))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
    })
}

fn monty_to_json_lossy(value: &MontyObject) -> Result<Value, ErrorObject> {
    serde_json::to_value(JsonMontyObject(value)).map_err(|err| {
        ErrorObject::new(
            "RESULT_NOT_SERIALIZABLE",
            format!("Monty value could not be converted to JSON: {err}"),
        )
    })
}

fn monty_to_json_checked(value: &MontyObject) -> Result<Value, ErrorObject> {
    if !is_plain_json(value) {
        return Err(ErrorObject::new(
            "RESULT_NOT_SERIALIZABLE",
            format!(
                "result contains non-JSON value of type {}.",
                value.type_name()
            ),
        )
        .with_hint(
            "Convert result to dict, list, str, int, float, bool, or None before returning.",
        ));
    }
    monty_to_json_lossy(value)
}

fn is_plain_json(value: &MontyObject) -> bool {
    match value {
        MontyObject::None
        | MontyObject::Bool(_)
        | MontyObject::Int(_)
        | MontyObject::BigInt(_)
        | MontyObject::String(_) => true,
        MontyObject::Float(value) => value.is_finite(),
        MontyObject::List(items) => items.iter().all(is_plain_json),
        MontyObject::Dict(pairs) => pairs
            .into_iter()
            .all(|(key, value)| matches!(key, MontyObject::String(_)) && is_plain_json(value)),
        _ => false,
    }
}

fn error_object_to_exception(error: &ErrorObject) -> MontyException {
    let exc_type = match error.code.as_str() {
        "PERMISSION_DENIED" => ExcType::PermissionError,
        "TYPE_ERROR" => ExcType::TypeError,
        "UNKNOWN_TOOL" => ExcType::NameError,
        "TIMEOUT_TOOL" | "TIMEOUT_WALL" => ExcType::TimeoutError,
        _ => ExcType::RuntimeError,
    };
    MontyException::new(exc_type, Some(format!("{}: {}", error.code, error.message)))
}

fn error_from_exception(error: MontyException, validation: bool) -> ErrorObject {
    let code = match error.exc_type() {
        ExcType::SyntaxError => "SYNTAX_ERROR",
        ExcType::TypeError if validation => "TYPE_ERROR",
        ExcType::NameError => "UNKNOWN_TOOL",
        ExcType::NotImplementedError | ExcType::ImportError | ExcType::ModuleNotFoundError => {
            "UNSUPPORTED_FEATURE"
        }
        ExcType::PermissionError => "PERMISSION_DENIED",
        ExcType::TimeoutError => "TIMEOUT_WALL",
        ExcType::MemoryError => "MEMORY_EXCEEDED",
        ExcType::RecursionError => "STACK_OVERFLOW",
        _ => "RUNTIME_ERROR",
    };
    let mut object = ErrorObject::new(code, error.summary());
    if code == "UNKNOWN_TOOL" {
        object.hint = Some(
            "Call list_tools() or describe_tool() to inspect registered functions.".to_owned(),
        );
    }
    if let Some(frame) = error.traceback().last() {
        object.span = Some(langshell_core::Span {
            line: frame.start.line,
            column: frame.start.column,
            end_line: Some(frame.end.line),
            end_column: Some(frame.end.column),
        });
    }
    object
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

fn static_validation_error(code: &str, registry: &ToolRegistry) -> Option<ErrorObject> {
    let unsupported = [
        "open(",
        "import os",
        "from os",
        "os.system",
        "subprocess",
        "import socket",
        "from socket",
        "import urllib",
        "from urllib",
        "import requests",
        "__class__",
        "__bases__",
        "__subclasses__",
    ];
    unsupported
        .iter()
        .find(|pattern| code.contains(**pattern))
        .map(|pattern| {
            ErrorObject::new(
                "UNSUPPORTED_FEATURE",
                format!("Use of {pattern:?} is not supported in the LangShell sandbox."),
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
                        format!("Function {name} is not registered in this session."),
                    )
                    .with_hint("Call list_tools() to inspect available capabilities.")
                })
        })
}

fn truncate_stdout(mut stdout: String, limits: &SessionLimits) -> String {
    let max = limits.max_stdout_bytes as usize;
    if stdout.len() > max {
        stdout.truncate(max);
    }
    stdout
}

fn metrics(started: Instant, memory_bytes: u64, external_calls_count: u32) -> Metrics {
    Metrics {
        duration_ms: elapsed_ms(started),
        memory_peak_bytes: memory_bytes,
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
    async fn runs_and_reuses_state() {
        let registry = ToolRegistry::new();
        let runtime = MontyRuntime::new(registry, SessionLimits::default());
        let first = RunRequest::new("s1", "cache = {'k': 1}").unwrap();
        assert_eq!(runtime.run(first).await.status, RunStatus::Ok);

        let second = RunRequest::new("s1", "result = cache['k'] + 1").unwrap();
        let result = runtime.run(second).await;
        assert_eq!(result.status, RunStatus::Ok);
        assert_eq!(result.result, Some(json!(2)));
    }

    #[tokio::test]
    async fn handles_async_external_function() {
        let mut registry = ToolRegistry::new();
        registry
            .register(RegisteredTool::asynchronous(
                Capability::new("fetch_json", "test fetch", SideEffect::Network),
                |ctx| {
                    Box::pin(async move {
                        let url = ctx.args.first().and_then(Value::as_str).unwrap_or_default();
                        Ok(json!({"url": url}))
                    })
                },
            ))
            .unwrap();
        let runtime = MontyRuntime::new(registry, SessionLimits::default());
        let code = r#"
import asyncio
data = await asyncio.gather(*(fetch_json(f"https://api.example.com/i/{i}") for i in range(3)))
result = {"n": len(data)}
"#;
        let result = runtime.run(RunRequest::new("s1", code).unwrap()).await;
        assert_eq!(result.status, RunStatus::Ok, "{result:?}");
        assert_eq!(result.result, Some(json!({"n": 3})));
        assert_eq!(result.metrics.external_calls_count, 3);
    }
}
