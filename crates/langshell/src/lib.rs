use std::{future::Future, path::PathBuf, sync::Arc};

pub use langshell_core::*;
pub use langshell_tools::{FileMount, ToolConfig};

use langshell_deno::{DenoRuntime, is_deno_snapshot};
use langshell_monty::MontyRuntime;
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct LangShell {
    monty: Arc<MontyRuntime>,
    deno: Arc<DenoRuntime>,
}

impl LangShell {
    pub fn builder() -> LangShellBuilder {
        LangShellBuilder::default()
    }

    pub async fn run(&self, request: RunRequest) -> RunResult {
        match request.language {
            Language::Python => self.monty.run(request).await,
            Language::TypeScript => self.deno.run(request).await,
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
        match language {
            Language::Python => {
                self.monty.create_session(session_id, None).await;
                Ok(())
            }
            Language::TypeScript => self.deno.create_session(session_id, None).await,
        }
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        let mut ids = self.monty.list_sessions().await;
        if let Ok(deno_ids) = self.deno.list_sessions().await {
            ids.extend(deno_ids);
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
        let removed_python = self.monty.destroy_session(&session_id).await;
        let removed_typescript = self.deno.destroy_session(&session_id).await?;
        Ok(removed_python || removed_typescript)
    }

    pub async fn snapshot_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<Vec<u8>, ErrorObject> {
        let session_id = SessionId::new(session_id)?;
        match self.monty.snapshot_session(&session_id).await {
            Ok(snapshot) => Ok(snapshot),
            Err(error) if error.code == "SESSION_NOT_FOUND" => {
                self.deno.snapshot_session(&session_id).await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn snapshot_session_with_language(
        &self,
        session_id: impl Into<String>,
        language: Language,
    ) -> Result<Vec<u8>, ErrorObject> {
        let session_id = SessionId::new(session_id)?;
        match language {
            Language::Python => self.monty.snapshot_session(&session_id).await,
            Language::TypeScript => self.deno.snapshot_session(&session_id).await,
        }
    }

    pub async fn restore_session(
        &self,
        snapshot: &[u8],
        session_id: Option<impl Into<String>>,
    ) -> Result<SessionId, ErrorObject> {
        let session_id = session_id.map(|id| SessionId::new(id.into())).transpose()?;
        if is_deno_snapshot(snapshot) {
            self.deno.restore_session(snapshot, session_id).await
        } else {
            self.monty.restore_session(snapshot, session_id).await
        }
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

#[derive(Debug, Clone)]
pub struct LangShellBuilder {
    registry: ToolRegistry,
    limits: SessionLimits,
    file_mounts: Vec<FileMount>,
    http_allowlist: Vec<String>,
}

impl Default for LangShellBuilder {
    fn default() -> Self {
        Self {
            registry: ToolRegistry::new(),
            limits: SessionLimits::default(),
            file_mounts: Vec::new(),
            http_allowlist: Vec::new(),
        }
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

    pub fn register_sync(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        side_effect: SideEffect,
        handler: impl Fn(ToolCallContext) -> ToolResult + Send + Sync + 'static,
    ) -> Result<Self, ErrorObject> {
        let capability = Capability::new(name, description, side_effect);
        self.registry
            .register(RegisteredTool::sync(capability, handler))?;
        Ok(self)
    }

    pub fn register_async<F, Fut>(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        side_effect: SideEffect,
        handler: F,
    ) -> Result<Self, ErrorObject>
    where
        F: Fn(ToolCallContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ToolResult> + Send + 'static,
    {
        let capability = Capability::new(name, description, side_effect);
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
        Ok(LangShell {
            monty: Arc::new(MontyRuntime::new(registry.clone(), limits.clone())),
            deno: Arc::new(DenoRuntime::new(registry, limits)),
        })
    }
}
