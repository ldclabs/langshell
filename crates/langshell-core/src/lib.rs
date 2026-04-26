use std::{collections::BTreeMap, fmt, future::Future, pin::Pin, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha3::{Digest, Sha3_256};

pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Result<Self, ErrorObject> {
        let id = id.into();
        if is_valid_session_id(&id) {
            Ok(Self(id))
        } else {
            Err(ErrorObject::new(
                "INVALID_SESSION_ID",
                "Session id must match ^[a-zA-Z0-9_\\-]{1,64}$.",
            ))
        }
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self("default".to_owned())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Language {
    #[default]
    Python,
    TypeScript,
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLimits {
    pub memory_mb: u32,
    pub cpu_ms: u32,
    pub wall_ms: u32,
    pub max_stdout_bytes: u32,
    pub max_external_calls: u32,
    pub max_stack_depth: u16,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            memory_mb: 64,
            cpu_ms: 2_000,
            wall_ms: 5_000,
            max_stdout_bytes: 65_536,
            max_external_calls: 32,
            max_stack_depth: 256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub created_at: u64,
    pub last_used_at: u64,
    pub language: Language,
    pub limits: SessionLimits,
    pub snapshot_version: u32,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self {
            id: SessionId::default(),
            created_at: 0,
            last_used_at: 0,
            language: Language::Python,
            limits: SessionLimits::default(),
            snapshot_version: SNAPSHOT_VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SideEffect {
    #[default]
    None,
    Read,
    Write,
    Network,
    Database,
    ExternalSystem,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityLimits {
    pub timeout_ms: u32,
    pub max_request_bytes: u32,
    pub max_response_bytes: u32,
    pub concurrency: u32,
    pub rate_per_min: Option<u32>,
}

impl Default for CapabilityLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 5_000,
            max_request_bytes: 1_048_576,
            max_response_bytes: 1_048_576,
            concurrency: 16,
            rate_per_min: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ApprovalPolicy {
    #[default]
    None,
    Auto { rule: String },
    Manual,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub side_effect: SideEffect,
    pub limits: CapabilityLimits,
    pub approval_policy: ApprovalPolicy,
    pub idempotent: bool,
}

impl Capability {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        side_effect: SideEffect,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "array"}),
            output_schema: json!({}),
            side_effect,
            limits: CapabilityLimits::default(),
            approval_policy: ApprovalPolicy::None,
            idempotent: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    pub session_id: SessionId,
    pub language: Language,
    pub code: String,
    pub inputs: Map<String, Value>,
    pub timeout_ms: Option<u32>,
    pub limits: Option<SessionLimits>,
    pub return_snapshot: bool,
    pub validate_only: bool,
}

impl RunRequest {
    pub fn new(
        session_id: impl Into<String>,
        code: impl Into<String>,
    ) -> Result<Self, ErrorObject> {
        Ok(Self {
            session_id: SessionId::new(session_id)?,
            language: Language::Python,
            code: code.into(),
            inputs: Map::new(),
            timeout_ms: None,
            limits: None,
            return_snapshot: false,
            validate_only: false,
        })
    }
}

impl Default for RunRequest {
    fn default() -> Self {
        Self {
            session_id: SessionId::default(),
            language: Language::Python,
            code: String::new(),
            inputs: Map::new(),
            timeout_ms: None,
            limits: None,
            return_snapshot: false,
            validate_only: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RunStatus {
    #[default]
    Ok,
    ValidationError,
    RuntimeError,
    Timeout,
    Cancelled,
    ResourceExhausted,
    PermissionDenied,
    WaitingForApproval,
    Interrupted,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Severity {
    Error,
    Warning,
    #[default]
    Info,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub line: u32,
    pub column: u32,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

impl Default for Span {
    fn default() -> Self {
        Self {
            line: 1,
            column: 1,
            end_line: None,
            end_column: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub fn error(error: &ErrorObject) -> Self {
        Self {
            severity: Severity::Error,
            code: error.code.clone(),
            message: error.message.clone(),
            hint: error.hint.clone(),
            span: error.span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CallStatus {
    #[default]
    Ok,
    Error,
    Timeout,
    Denied,
    ApprovalPending,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
    pub span: Option<Span>,
}

impl ErrorObject {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: None,
            span: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCallRecord {
    pub name: String,
    pub side_effect: SideEffect,
    pub duration_ms: u32,
    pub status: CallStatus,
    pub request_digest: String,
    pub response_digest: Option<String>,
    pub error: Option<ErrorObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Metrics {
    pub duration_ms: u32,
    pub memory_peak_bytes: u64,
    pub instructions: u64,
    pub external_calls_count: u32,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub status: RunStatus,
    pub result: Option<Value>,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Vec<Diagnostic>,
    pub external_calls: Vec<ExternalCallRecord>,
    pub snapshot_id: Option<String>,
    pub metrics: Metrics,
    pub error: Option<ErrorObject>,
}

impl RunResult {
    pub fn ok(result: Option<Value>, stdout: String, metrics: Metrics) -> Self {
        Self {
            status: RunStatus::Ok,
            result,
            stdout,
            stderr: String::new(),
            diagnostics: Vec::new(),
            external_calls: Vec::new(),
            snapshot_id: None,
            metrics,
            error: None,
        }
    }

    pub fn error(status: RunStatus, error: ErrorObject, stdout: String, metrics: Metrics) -> Self {
        Self {
            status,
            result: None,
            stdout,
            stderr: String::new(),
            diagnostics: vec![Diagnostic::error(&error)],
            external_calls: Vec::new(),
            snapshot_id: None,
            metrics,
            error: Some(error),
        }
    }
}

impl Default for RunResult {
    fn default() -> Self {
        Self::ok(None, String::new(), Metrics::default())
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub name: String,
    pub args: Vec<Value>,
    pub kwargs: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub type ToolResult = Result<Value, ToolError>;
pub type ToolFuture = Pin<Box<dyn Future<Output = ToolResult> + Send + 'static>>;
pub type ToolFn = dyn Fn(ToolCallContext) -> ToolFuture + Send + Sync + 'static;

#[derive(Clone)]
pub struct RegisteredTool {
    pub capability: Capability,
    pub async_mode: bool,
    handler: Arc<ToolFn>,
}

impl fmt::Debug for RegisteredTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisteredTool")
            .field("capability", &self.capability)
            .field("async_mode", &self.async_mode)
            .finish_non_exhaustive()
    }
}

impl RegisteredTool {
    pub fn sync(
        capability: Capability,
        handler: impl Fn(ToolCallContext) -> ToolResult + Send + Sync + 'static,
    ) -> Self {
        let handler = Arc::new(move |ctx| {
            let result = handler(ctx);
            Box::pin(async move { result }) as ToolFuture
        });
        Self {
            capability,
            async_mode: false,
            handler,
        }
    }

    pub fn asynchronous(
        capability: Capability,
        handler: impl Fn(ToolCallContext) -> ToolFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            capability,
            async_mode: true,
            handler: Arc::new(handler),
        }
    }

    pub fn call(&self, ctx: ToolCallContext) -> ToolFuture {
        (self.handler)(ctx)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: RegisteredTool) -> Result<(), ErrorObject> {
        if !is_python_identifier(&tool.capability.name) {
            return Err(ErrorObject::new(
                "INVALID_TOOL_NAME",
                format!(
                    "Capability name '{}' is not a valid Python identifier.",
                    tool.capability.name
                ),
            ));
        }
        self.tools.insert(tool.capability.name.clone(), tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn capabilities(&self) -> Vec<Capability> {
        self.tools
            .values()
            .map(|tool| tool.capability.clone())
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

pub fn is_python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub fn digest_json(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha3_256::digest(bytes);
    to_hex(&digest)
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    let digest = Sha3_256::digest(bytes);
    to_hex(&digest)
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_session_ids() {
        assert!(SessionId::new("agent-123_ok").is_ok());
        assert!(SessionId::new("bad/slash").is_err());
        assert!(SessionId::new("").is_err());
    }

    #[test]
    fn validates_python_identifiers() {
        assert!(is_python_identifier("fetch_json"));
        assert!(!is_python_identifier("1_fetch"));
        assert!(!is_python_identifier("fetch-json"));
    }
}
