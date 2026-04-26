use std::{future::Future, path::PathBuf, sync::Arc};

pub use langshell_core::*;
pub use langshell_tools::{FileMount, ToolConfig};

use langshell_monty::MontyRuntime;
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct LangShell {
    runtime: Arc<MontyRuntime>,
}

impl LangShell {
    pub fn builder() -> LangShellBuilder {
        LangShellBuilder::default()
    }

    pub async fn run(&self, request: RunRequest) -> RunResult {
        self.runtime.run(request).await
    }

    pub async fn validate(&self, mut request: RunRequest) -> RunResult {
        request.validate_only = true;
        self.runtime.run(request).await
    }

    pub fn session(&self, session_id: impl Into<String>) -> SessionHandle {
        SessionHandle {
            runtime: self.runtime.clone(),
            session_id: session_id.into(),
        }
    }

    pub async fn create_session(&self, session_id: impl Into<String>) -> Result<(), ErrorObject> {
        self.runtime
            .create_session(SessionId::new(session_id)?, None)
            .await;
        Ok(())
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        self.runtime.list_sessions().await
    }

    pub async fn destroy_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<bool, ErrorObject> {
        Ok(self
            .runtime
            .destroy_session(&SessionId::new(session_id)?)
            .await)
    }

    pub async fn snapshot_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<Vec<u8>, ErrorObject> {
        self.runtime
            .snapshot_session(&SessionId::new(session_id)?)
            .await
    }

    pub async fn restore_session(
        &self,
        snapshot: &[u8],
        session_id: Option<impl Into<String>>,
    ) -> Result<SessionId, ErrorObject> {
        let session_id = session_id.map(|id| SessionId::new(id.into())).transpose()?;
        self.runtime.restore_session(snapshot, session_id).await
    }
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    runtime: Arc<MontyRuntime>,
    session_id: String,
}

impl SessionHandle {
    pub fn run(&self, code: impl Into<String>) -> RunBuilder {
        RunBuilder {
            runtime: self.runtime.clone(),
            request: RunRequest {
                session_id: SessionId(self.session_id.clone()),
                language: Language::Python,
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
    runtime: Arc<MontyRuntime>,
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
        self.runtime.run(self.request).await
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
        Ok(LangShell {
            runtime: Arc::new(MontyRuntime::new(self.registry, self.limits)),
        })
    }
}
