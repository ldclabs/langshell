use std::sync::Mutex;

use langshell::{
    ErrorObject, LangShell, Language, LanguageRuntime, Metrics, RunRequest, RunResult, RunStatus,
    RuntimeFuture, SessionId, SessionLimits, ToolRegistry,
};
use serde_json::json;

#[derive(Debug)]
struct FakeRuntime {
    language: Language,
    sessions: Mutex<Vec<SessionId>>,
}

impl FakeRuntime {
    fn new(_registry: ToolRegistry, _limits: SessionLimits) -> Self {
        Self {
            language: Language::Python,
            sessions: Mutex::new(Vec::new()),
        }
    }
}

impl LanguageRuntime for FakeRuntime {
    fn language(&self) -> Language {
        self.language
    }

    fn create_session(
        &self,
        session_id: SessionId,
        _limits: Option<SessionLimits>,
    ) -> RuntimeFuture<'_, Result<(), ErrorObject>> {
        let result = self
            .sessions
            .lock()
            .map(|mut sessions| sessions.push(session_id))
            .map_err(|err| ErrorObject::new("RUNTIME_ERROR", err.to_string()));
        Box::pin(async move { result })
    }

    fn run(&self, request: RunRequest) -> RuntimeFuture<'_, RunResult> {
        let result = RunResult::ok(
            Some(json!({
                "language": language_name(request.language),
                "code": request.code,
            })),
            String::new(),
            Metrics::default(),
        );
        Box::pin(async move { result })
    }

    fn destroy_session(
        &self,
        session_id: SessionId,
    ) -> RuntimeFuture<'_, Result<bool, ErrorObject>> {
        let result = self
            .sessions
            .lock()
            .map(|mut sessions| {
                let before = sessions.len();
                sessions.retain(|id| id != &session_id);
                sessions.len() != before
            })
            .map_err(|err| ErrorObject::new("RUNTIME_ERROR", err.to_string()));
        Box::pin(async move { result })
    }

    fn list_sessions(&self) -> RuntimeFuture<'_, Result<Vec<SessionId>, ErrorObject>> {
        let result = self
            .sessions
            .lock()
            .map(|sessions| sessions.clone())
            .map_err(|err| ErrorObject::new("RUNTIME_ERROR", err.to_string()));
        Box::pin(async move { result })
    }

    fn snapshot_session(
        &self,
        session_id: SessionId,
    ) -> RuntimeFuture<'_, Result<Vec<u8>, ErrorObject>> {
        let snapshot = format!("fake-runtime:{}", session_id.0).into_bytes();
        Box::pin(async move { Ok(snapshot) })
    }

    fn restore_session(
        &self,
        _snapshot: Vec<u8>,
        session_id: Option<SessionId>,
    ) -> RuntimeFuture<'_, Result<SessionId, ErrorObject>> {
        let session_id = session_id.unwrap_or_else(|| SessionId("restored".to_owned()));
        let result = self
            .sessions
            .lock()
            .map(|mut sessions| {
                sessions.push(session_id.clone());
                session_id
            })
            .map_err(|err| ErrorObject::new("RUNTIME_ERROR", err.to_string()));
        Box::pin(async move { result })
    }

    fn can_restore_snapshot(&self, snapshot: &[u8]) -> bool {
        snapshot.starts_with(b"fake-runtime:")
    }
}

#[test]
fn builder_requires_runtime() {
    let error = LangShell::builder().build().expect_err("build should fail");
    assert_eq!(error.code, "INVALID_ARGUMENT");
}

#[test]
fn builder_rejects_duplicate_runtime_languages() {
    let error = LangShell::builder()
        .runtime(FakeRuntime::new)
        .runtime(FakeRuntime::new)
        .build()
        .expect_err("duplicate runtime should fail");
    assert_eq!(error.code, "INVALID_ARGUMENT");
}

#[tokio::test]
async fn sdk_delegates_to_registered_runtime() {
    let shell = LangShell::builder()
        .runtime(FakeRuntime::new)
        .build()
        .unwrap();

    let result = shell.session("sdk-e2e").run("result = 1").execute().await;

    assert_eq!(result.status, RunStatus::Ok, "{result:?}");
    assert_eq!(
        result.result,
        Some(json!({"language": "python", "code": "result = 1"}))
    );
}

#[tokio::test]
async fn missing_runtime_returns_validation_error() {
    let shell = LangShell::builder()
        .runtime(FakeRuntime::new)
        .build()
        .unwrap();
    let mut request = RunRequest::new("sdk-e2e", "result = 1").unwrap();
    request.language = Language::TypeScript;

    let result = shell.run(request).await;

    assert_eq!(result.status, RunStatus::ValidationError, "{result:?}");
    assert_eq!(result.error.unwrap().code, "UNSUPPORTED_FEATURE");
}

fn language_name(language: Language) -> &'static str {
    match language {
        Language::Python => "python",
        Language::TypeScript => "typescript",
    }
}
