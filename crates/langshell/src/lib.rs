use std::{fmt, future::Future, path::PathBuf, sync::Arc};

pub use langshell_core::*;
pub use langshell_tools::{FileMount, ToolConfig};

use serde_json::{Map, Value};

type RuntimeFactory =
    dyn Fn(ToolRegistry, SessionLimits) -> Arc<dyn LanguageRuntime> + Send + Sync + 'static;

#[derive(Debug, Clone)]
pub struct LangShell {
    runtimes: Vec<Arc<dyn LanguageRuntime>>,
}

impl LangShell {
    pub fn builder() -> LangShellBuilder {
        LangShellBuilder::default()
    }

    pub async fn run(&self, request: RunRequest) -> RunResult {
        let language = request.language;
        match self.runtime_for(language) {
            Ok(runtime) => runtime.run(request).await,
            Err(error) => RunResult::error(
                RunStatus::ValidationError,
                error,
                String::new(),
                Metrics::default(),
            ),
        }
    }

    pub async fn validate(&self, mut request: RunRequest) -> RunResult {
        request.validate_only = true;
        self.run(request).await
    }

    pub fn session(&self, session_id: impl Into<String>) -> SessionHandle {
        self.session_with_language(session_id, Language::Python)
    }

    pub fn typescript_session(&self, session_id: impl Into<String>) -> SessionHandle {
        self.session_with_language(session_id, Language::TypeScript)
    }

    pub fn session_with_language(
        &self,
        session_id: impl Into<String>,
        language: Language,
    ) -> SessionHandle {
        SessionHandle {
            shell: self.clone(),
            session_id: session_id.into(),
            language,
        }
    }

    pub async fn create_session(&self, session_id: impl Into<String>) -> Result<(), ErrorObject> {
        self.create_session_with_language(session_id, Language::Python)
            .await
    }

    pub async fn create_session_with_language(
        &self,
        session_id: impl Into<String>,
        language: Language,
    ) -> Result<(), ErrorObject> {
        let session_id = SessionId::new(session_id)?;
        self.runtime_for(language)?
            .create_session(session_id, None)
            .await
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        let mut ids = Vec::new();
        for runtime in &self.runtimes {
            if let Ok(runtime_ids) = runtime.list_sessions().await {
                ids.extend(runtime_ids);
            }
        }
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        ids.dedup_by(|a, b| a.0 == b.0);
        ids
    }

    pub async fn destroy_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<bool, ErrorObject> {
        let session_id = SessionId::new(session_id)?;
        let mut removed = false;
        for runtime in &self.runtimes {
            removed |= runtime.destroy_session(session_id.clone()).await?;
        }
        Ok(removed)
    }

    pub async fn snapshot_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<Vec<u8>, ErrorObject> {
        let session_id = SessionId::new(session_id)?;
        let mut not_found = None;
        for runtime in &self.runtimes {
            match runtime.snapshot_session(session_id.clone()).await {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) if error.code == "SESSION_NOT_FOUND" => not_found = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(not_found.unwrap_or_else(|| no_runtime_registered_error(None)))
    }

    pub async fn snapshot_session_with_language(
        &self,
        session_id: impl Into<String>,
        language: Language,
    ) -> Result<Vec<u8>, ErrorObject> {
        let session_id = SessionId::new(session_id)?;
        self.runtime_for(language)?
            .snapshot_session(session_id)
            .await
    }

    pub async fn restore_session(
        &self,
        snapshot: &[u8],
        session_id: Option<impl Into<String>>,
    ) -> Result<SessionId, ErrorObject> {
        let session_id = session_id.map(|id| SessionId::new(id.into())).transpose()?;
        if let Some(runtime) = self
            .runtimes
            .iter()
            .find(|runtime| runtime.can_restore_snapshot(snapshot))
        {
            return runtime.restore_session(snapshot.to_vec(), session_id).await;
        }

        if self.runtimes.len() == 1 {
            return self.runtimes[0]
                .restore_session(snapshot.to_vec(), session_id)
                .await;
        }

        Err(ErrorObject::new(
            "SNAPSHOT_CORRUPT",
            "No registered language runtime can restore this snapshot.",
        )
        .with_hint(
            "Register the backend that created the snapshot before calling restore_session.",
        ))
    }

    fn runtime_for(&self, language: Language) -> Result<&dyn LanguageRuntime, ErrorObject> {
        self.runtimes
            .iter()
            .find(|runtime| runtime.language() == language)
            .map(|runtime| runtime.as_ref())
            .ok_or_else(|| no_runtime_registered_error(Some(language)))
    }
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    shell: LangShell,
    session_id: String,
    language: Language,
}

impl SessionHandle {
    pub fn with_language(&self, language: Language) -> Self {
        Self {
            shell: self.shell.clone(),
            session_id: self.session_id.clone(),
            language,
        }
    }

    pub fn run(&self, code: impl Into<String>) -> RunBuilder {
        RunBuilder {
            shell: self.shell.clone(),
            request: RunRequest {
                session_id: SessionId(self.session_id.clone()),
                language: self.language,
                code: code.into(),
                inputs: Map::new(),
                timeout_ms: None,
                limits: None,
                return_snapshot: false,
                validate_only: false,
            },
        }
    }

    pub fn validate(&self, code: impl Into<String>) -> RunBuilder {
        let mut builder = self.run(code);
        builder.request.validate_only = true;
        builder
    }
}

#[derive(Debug, Clone)]
pub struct RunBuilder {
    shell: LangShell,
    request: RunRequest,
}

impl RunBuilder {
    pub fn input(mut self, key: impl Into<String>, value: Value) -> Self {
        self.request.inputs.insert(key.into(), value);
        self
    }

    pub fn inputs(mut self, inputs: Map<String, Value>) -> Self {
        self.request.inputs = inputs;
        self
    }

    pub fn timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.request.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn return_snapshot(mut self, enabled: bool) -> Self {
        self.request.return_snapshot = enabled;
        self
    }

    pub async fn execute(self) -> RunResult {
        self.shell.run(self.request).await
    }
}

#[derive(Clone, Default)]
pub struct LangShellBuilder {
    registry: ToolRegistry,
    limits: SessionLimits,
    file_mounts: Vec<FileMount>,
    http_allowlist: Vec<String>,
    runtime_factories: Vec<Arc<RuntimeFactory>>,
}

impl fmt::Debug for LangShellBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LangShellBuilder")
            .field("registry", &self.registry)
            .field("limits", &self.limits)
            .field("file_mounts", &self.file_mounts)
            .field("http_allowlist", &self.http_allowlist)
            .field("runtime_factories", &self.runtime_factories.len())
            .finish()
    }
}

impl LangShellBuilder {
    pub fn limits(mut self, limits: SessionLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn memory_limit_mb(mut self, memory_mb: u32) -> Self {
        self.limits.memory_mb = memory_mb;
        self
    }

    pub fn timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.limits.wall_ms = timeout_ms;
        self
    }

    pub fn mount_readonly(
        mut self,
        virtual_path: impl Into<String>,
        host_path: impl Into<PathBuf>,
    ) -> Self {
        self.file_mounts
            .push(FileMount::readonly(virtual_path, host_path));
        self
    }

    pub fn mount_readwrite(
        mut self,
        virtual_path: impl Into<String>,
        host_path: impl Into<PathBuf>,
    ) -> Self {
        self.file_mounts
            .push(FileMount::readwrite(virtual_path, host_path));
        self
    }

    pub fn allow_http_host(mut self, host: impl Into<String>) -> Self {
        self.http_allowlist.push(host.into());
        self
    }

    pub fn runtime<R>(
        mut self,
        factory: impl Fn(ToolRegistry, SessionLimits) -> R + Send + Sync + 'static,
    ) -> Self
    where
        R: LanguageRuntime + 'static,
    {
        self.runtime_factories
            .push(Arc::new(move |registry, limits| {
                let runtime: Arc<dyn LanguageRuntime> = Arc::new(factory(registry, limits));
                runtime
            }));
        self
    }

    pub fn register_sync(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        side_effect: SideEffect,
        input_schema: Value,
        output_schema: Value,
        handler: impl Fn(ToolCallContext) -> ToolResult + Send + Sync + 'static,
    ) -> Result<Self, ErrorObject> {
        let capability = Capability::new(name, description, side_effect)
            .with_input_schema(input_schema)
            .with_output_schema(output_schema);
        self.registry
            .register(RegisteredTool::sync(capability, handler))?;
        Ok(self)
    }

    pub fn register_async<F, Fut>(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        side_effect: SideEffect,
        input_schema: Value,
        output_schema: Value,
        handler: F,
    ) -> Result<Self, ErrorObject>
    where
        F: Fn(ToolCallContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ToolResult> + Send + 'static,
    {
        let capability = Capability::new(name, description, side_effect)
            .with_input_schema(input_schema)
            .with_output_schema(output_schema);
        self.registry
            .register(RegisteredTool::asynchronous(capability, move |ctx| {
                Box::pin(handler(ctx)) as ToolFuture
            }))?;
        Ok(self)
    }

    pub fn register_tool(mut self, tool: RegisteredTool) -> Result<Self, ErrorObject> {
        self.registry.register(tool)?;
        Ok(self)
    }

    pub fn build(mut self) -> Result<LangShell, ErrorObject> {
        langshell_tools::register_builtin_tools(
            &mut self.registry,
            ToolConfig {
                file_mounts: self.file_mounts,
                http_allowlist: self.http_allowlist,
            },
        )?;
        let registry = self.registry;
        let limits = self.limits;
        let mut runtimes = Vec::<Arc<dyn LanguageRuntime>>::new();
        for factory in self.runtime_factories {
            let runtime = factory(registry.clone(), limits.clone());
            let language = runtime.language();
            if runtimes
                .iter()
                .any(|existing| existing.language() == language)
            {
                return Err(ErrorObject::new(
                    "INVALID_ARGUMENT",
                    format!(
                        "A runtime for language {} is already registered.",
                        language_name(language)
                    ),
                ));
            }
            runtimes.push(runtime);
        }
        if runtimes.is_empty() {
            return Err(no_runtime_registered_error(None));
        }
        Ok(LangShell { runtimes })
    }
}

fn no_runtime_registered_error(language: Option<Language>) -> ErrorObject {
    match language {
        Some(language) => ErrorObject::new(
            "UNSUPPORTED_FEATURE",
            format!(
                "No language runtime is registered for {}.",
                language_name(language)
            ),
        )
        .with_hint("Register a backend with LangShell::builder().runtime(...) before build()."),
        None => ErrorObject::new(
            "INVALID_ARGUMENT",
            "At least one language runtime must be registered.",
        )
        .with_hint(
            "Call .runtime(langshell_monty::MontyRuntime::new) or .runtime(langshell_deno::DenoRuntime::new) before build().",
        ),
    }
}

fn language_name(language: Language) -> &'static str {
    match language {
        Language::Python => "python",
        Language::TypeScript => "typescript",
    }
}
