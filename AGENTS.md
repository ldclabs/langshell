# LangShell 产品与工程契约

**版本**：1.3（2026-04-30）
**状态**：V1 开发契约
**项目定位**：专为 AI Agent 设计的状态持久、能力受控、安全沙箱代码执行层
**当前范围**：Python / Monty 与 TypeScript / Deno 双后端；CBOR 快照；AST 静态校验；schema 化 capability registry

> LangShell 仍处于开发阶段。稳定发布前，本地 session、snapshot、缓存和内部数据结构都不承诺向后兼容。实现可以直接淘汰旧格式；遇到旧数据应返回明确错误，而不是尝试迁移。

## 1. 产品定位

LangShell 把 Agent 的多步工具使用变成一段可验证、可复用、可恢复、可审计的程序。Agent 用代码表达循环、条件、并发、缓存、重试、数据变换和多步推理；宿主通过 capability registry 决定这段代码能触达哪些外部系统。

核心路径：

```text
AI tokens -> sandboxed code -> mediated capabilities -> structured result -> resumable state
```

LangShell 同时服务三类使用者：

- **AI Agent**：直接运行 Python 或 TypeScript 逻辑，保留中间状态，拿到结构化结果和可修复错误。
- **Agent 框架开发者**：用稳定 Rust SDK、CLI 或 JSON-RPC daemon 嵌入执行能力，注册外部函数，控制权限、超时、资源和审计。
- **平台 / 安全负责人**：保持默认零权限，让每个副作用都经过命名 capability、审计记录、资源限制和策略检查。

LangShell 不是完整 CPython、不是通用 OS shell、不是容器平台，也不鼓励绕过宿主权限模型。它的定位是 Agent 的安全程序化工具调用层。

## 2. 设计原则

- **Code is the interface**：Agent 主要输出代码，不需要为每个微动作设计新 tool schema。
- **Session is the unit**：状态、权限、限制、快照、审计和生命周期都以 session 为单位。
- **Capabilities over permissions**：默认没有文件、网络、环境变量、子进程或数据库权限；外部能力必须由宿主显式注册。
- **Every side effect is mediated**：文件、HTTP、数据库、邮件、业务 API 等副作用必须经过 registered tool。
- **Errors are for agents**：错误码、hint 和 span 应稳定、结构化、可用于自动修复。
- **Small safe core, extensible edge**：核心 runtime 保持小而安全；外部能力通过 registry、SDK 和后续 plugin 扩展。
- **Development data is disposable**：开发期快照和本地 session 文件只服务当前代码版本，不做旧格式迁移。

## 3. 当前开发基线

- Python 后端：`langshell-monty`，基于 Pydantic Monty，支持持久 REPL 状态、top-level await、async capability、stdout/stderr 捕获、result 捕获和 CBOR snapshot。
- TypeScript 后端：`langshell-deno`，基于 Deno/V8，支持持久 globals、async capability、stdout/stderr 捕获、result 捕获和带类型标签的 CBOR snapshot。
- 静态校验：Python 使用 Ruff parser AST；TypeScript 使用 `deno_ast` / SWC AST。禁止危险 import、危险全局、反射逃逸属性、动态执行入口和未注册的 capability-like 调用。
- Snapshot：当前格式是 CBOR v2 envelope。旧 JSON 或旧版本 snapshot 不迁移。
- Capability registry：`register_sync` / `register_async` 必须提供 `input_schema` 和 `output_schema`。
- Discovery：`list_tools`、`describe_tool`、`current_policy` 返回能力说明、schema、限制和副作用等级。
- CLI：`run`、`validate`、`repl`、`daemon`、`session`、`tools` 输出稳定 JSON。
- Daemon：当前传输为 Unix socket 上的 line-delimited JSON-RPC 2.0。

## 4. 功能契约

### 4.1 执行核心

- 支持 Python 子集和 TypeScript 两种 language backend。
- 支持变量、函数、控制流、列表 / 字典、异常、类型提示、`async` / `await`。
- 支持 top-level await。
- 支持 stdout / stderr 捕获，并按 `max_stdout_bytes` 截断。
- 支持 validate / dry-run，不产生副作用。
- 支持 per-session 和 per-run limits：内存、墙钟时间、输出大小、外部调用次数、栈深。
- 支持取消和超时错误的稳定返回。

### 4.2 Session

- `session_id` 必须匹配 `^[a-zA-Z0-9_\-]{1,64}$`。
- 同一 session 保留变量、函数、缓存、已注册 tool stubs、runtime globals 和 snapshot 元数据。
- 生命周期操作：create、run、validate、snapshot、restore、destroy、list。
- 后续开发：fork、diff、reset、cancel、durable store。

### 4.3 Capability Registry

每个 capability 必须包含：

- `name`：沙箱内函数名，必须是合法 Python/TypeScript 标识符。
- `description`：给 Agent 看的说明。
- `input_schema`：JSON Schema draft-2020-12，用于描述 positional args / kwargs。
- `output_schema`：JSON Schema draft-2020-12，用于描述返回值。
- `side_effect`：`none`、`read`、`write`、`network`、`database`、`external_system`。
- `limits`：单次调用超时、请求/响应大小、并发、速率限制。
- `approval_policy`：`none`、`auto`、`manual`。
- `idempotent`：是否适合安全重试。

外部函数必须支持 sync 和 async 两种注册形式。Runtime 负责把它们暴露为沙箱内普通函数或 awaitable 函数，并记录 external call 审计项。

### 4.4 内置能力

- 文件：`read_text`、`write_text`、`list_dir`，仅限授权虚拟挂载。
- HTTP helper：`fetch_text`、`fetch_json`，执行 schema 和 allowlist 检查；默认构建不包含真实网络 transport，宿主可覆盖注册。
- Discovery：`list_tools`、`describe_tool`、`current_policy`。

## 5. 安全模型

- 默认禁止宿主文件系统、环境变量、网络、子进程、动态库、危险 import 和反射逃逸。
- 文件访问只能通过虚拟挂载或注册函数完成。
- 沙箱路径使用 POSIX 风格，例如 `/workspace/input.json`。
- 路径解析必须阻止 `..`、符号链接逃逸、空字节、大小写规避和挂载边界绕过。
- 网络访问必须通过注册函数，并支持域名 allowlist、协议限制、代理、超时、重试和响应大小限制。
- 外部函数是主要风险边界，必须记录调用名称、参数摘要、结果摘要、耗时、错误和审批结果。
- Snapshot 只从可信宿主路径加载，不把 untrusted snapshot 当作安全输入。
- 开发期旧 snapshot 不迁移；生产稳定前不做旧数据兼容保证。

## 6. Agent-facing 示例

### 6.1 Python

```python
import json

async def main():
    items = await fetch_json("https://api.example.com/items")
    selected = [item for item in items if item.get("score", 0) >= 0.8]
    await write_text("/workspace/selected.json", json.dumps(selected))
    return {"selected": len(selected), "total": len(items)}

result = await main()
print(json.dumps(result))
```

### 6.2 TypeScript

```ts
const items = await Promise.all([
  fetch_json("https://api.example.com/i/1"),
  fetch_json("https://api.example.com/i/2"),
]);

result = { loaded: items.length };
```

### 6.3 能力发现

```python
tools = list_tools()
info = describe_tool("fetch_json")
policy = current_policy()
```

## 7. CLI 与 JSON-RPC

### 7.1 CLI

```bash
langshell run -e 'result = sum(range(10))' --json
langshell validate -e 'open("/etc/passwd")' --json
langshell repl --session-id agent-123 --language python
langshell session list
langshell session snapshot agent-123 --out /tmp/agent.cbor
langshell session restore --from /tmp/agent.cbor --session-id restored
langshell tools list --session-id agent-123
langshell daemon --listen unix:///tmp/langshell.sock
```

CLI 默认输出 JSON。本地持久 session 文件使用当前 CBOR 格式，扩展名为 `.cbor`。

### 7.2 JSON-RPC

核心方法：

- `session.create`
- `session.run`
- `session.validate`
- `session.snapshot`
- `session.restore`
- `session.destroy`
- `tools.list`
- `tools.describe`
- `policy.get`

请求示例：

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "method": "session.run",
  "params": {
    "session_id": "agent-123",
    "language": "python",
    "code": "result = sum(range(10))",
    "return_snapshot": true
  }
}
```

## 8. Rust SDK

宿主必须显式选择 runtime backend，并显式注册 capability schema。

```rust
use langshell::{LangShell, SideEffect};
use langshell_monty::MontyRuntime;
use serde_json::{Value, json};

let shell = LangShell::builder()
    .runtime(MontyRuntime::new)
    .memory_limit_mb(64)
    .timeout_ms(5_000)
    .mount_readonly("/workspace", host_workspace)
    .register_async(
        "fetch_json",
        "Fetch JSON from an approved URL.",
        SideEffect::Network,
        json!({"type": "array", "prefixItems": [{"type": "string"}], "minItems": 1, "maxItems": 1}),
        json!({"type": "object"}),
        fetch_json_fn,
    )?
    .build()?;
```

## 9. 仓库结构与模块边界

```text
langshell/
├── monty/                  # submodule: pydantic/monty
├── deno/                   # submodule: denoland/deno
├── crates/
│   ├── langshell-core/     # session、policy、registry、snapshot、diagnostics 抽象
│   ├── langshell-monty/    # Python / Monty runtime
│   ├── langshell-deno/     # TypeScript / Deno runtime
│   ├── langshell-tools/    # 内置 capability helper
│   ├── langshell-cli/      # CLI 与 JSON-RPC daemon
│   └── langshell/          # 公共 Rust SDK
├── docs/
└── examples/
```

边界规则：

- `langshell-core` 不依赖具体执行引擎，只定义 trait 与数据类型。
- `langshell-monty` 封装所有 Monty API 使用。
- `langshell-deno` 封装所有 Deno/V8 API 使用。
- `langshell` 不依赖具体 backend crate；宿主通过 `LanguageRuntime` trait 注册 backend。
- `langshell-tools` 中每个能力应保持独立模块化，便于单独启用、禁用和审计。
- CLI 可以组装具体 backend；SDK 保持 backend-neutral。

## 10. 关键数据结构

所有面向 Agent / 客户端的枚举使用 snake_case 字符串标签序列化。公共类型必须实现 `Serialize + Deserialize + Clone + Debug`。

```rust
pub struct SessionId(pub String);

pub struct SessionLimits {
    pub memory_mb: u32,
    pub cpu_ms: u32,
    pub wall_ms: u32,
    pub max_stdout_bytes: u32,
    pub max_external_calls: u32,
    pub max_stack_depth: u16,
}

pub enum SideEffect {
    None,
    Read,
    Write,
    Network,
    Database,
    ExternalSystem,
}

pub struct Capability {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub side_effect: SideEffect,
    pub limits: CapabilityLimits,
    pub approval_policy: ApprovalPolicy,
    pub idempotent: bool,
}

pub struct RunRequest {
    pub session_id: SessionId,
    pub language: Language,
    pub code: String,
    pub inputs: serde_json::Map<String, serde_json::Value>,
    pub timeout_ms: Option<u32>,
    pub limits: Option<SessionLimits>,
    pub return_snapshot: bool,
    pub validate_only: bool,
}

pub struct RunResult {
    pub status: RunStatus,
    pub result: Option<serde_json::Value>,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Vec<Diagnostic>,
    pub external_calls: Vec<ExternalCallRecord>,
    pub snapshot_id: Option<String>,
    pub metrics: Metrics,
    pub error: Option<ErrorObject>,
}
```

## 11. 结果捕获优先级

1. 执行结束时全局作用域存在 `result`，则使用 `result` 作为 `RunResult.result`。
2. 否则如果最后一条语句是表达式，且 backend 支持表达式捕获，则使用最后表达式值。
3. 否则 `RunResult.result = null`，调用方可回退 stdout。

约束：

- `result` 必须是 JSON 可序列化值。
- 不可序列化时返回 `RESULT_NOT_SERIALIZABLE`。
- stdout / stderr 按 `max_stdout_bytes` 截断，并附 `STDOUT_EXCEEDED` warning diagnostic。

## 12. Snapshot 格式

当前 snapshot 使用 `ciborium` 编码的 CBOR envelope。

通用字段：

- `magic`：backend-specific magic，例如 `langshell-snapshot/v2` 或 `langshell-deno-snapshot/v2`。
- `version`：等于 `langshell_core::SNAPSHOT_VERSION`。
- `session_id`。
- `limits`。
- `capability_digest`：当前注册 capability 名称集合摘要。

Backend 字段：

- Monty：`repl_dump`，由 `MontyRepl::dump()` 生成。
- Deno：`globals`，由 JS-side snapshot helper 生成；支持 `bigint`、`Uint8Array`、`Map`、`Set`、`Date` 类型标签。

加载规则：

- 只接受当前 `magic` 和当前 `SNAPSHOT_VERSION`。
- 不加载旧 JSON 或旧版本快照。
- capability digest 必须匹配当前 host 注册集，否则返回 `SNAPSHOT_CAPABILITY_MISMATCH`。
- 损坏或非 CBOR 数据返回 `SNAPSHOT_CORRUPT`。

不持久化：实时 socket、文件句柄、stdout 缓冲、密钥、后台 task。

## 13. 错误码

错误对象统一形状：

```json
{ "code": "UNKNOWN_TOOL", "message": "...", "hint": "...", "span": { "line": 1, "column": 14 } }
```

稳定错误码：

| Code                                                                                 | 触发条件                                 | Status               |
| ------------------------------------------------------------------------------------ | ---------------------------------------- | -------------------- |
| `SYNTAX_ERROR`                                                                       | 解析失败                                 | validation_error     |
| `TYPE_ERROR`                                                                         | 类型或参数形状错误                       | validation_error     |
| `UNKNOWN_TOOL`                                                                       | 调用未注册 capability                    | validation_error     |
| `UNSUPPORTED_FEATURE`                                                                | 使用不支持语法、模块、危险全局或逃逸模式 | validation_error     |
| `RESULT_NOT_SERIALIZABLE`                                                            | `result` 不是 JSON 可序列化              | validation_error     |
| `PERMISSION_DENIED`                                                                  | 未授权路径或能力                         | permission_denied    |
| `WAITING_FOR_APPROVAL`                                                               | 副作用需审批                             | waiting_for_approval |
| `TIMEOUT_WALL` / `TIMEOUT_CPU` / `TIMEOUT_TOOL`                                      | 各类超时                                 | timeout              |
| `CANCELLED`                                                                          | 主动取消                                 | cancelled            |
| `MEMORY_EXCEEDED` / `STDOUT_EXCEEDED` / `EXTERNAL_CALLS_EXCEEDED` / `STACK_OVERFLOW` | 资源超限                                 | resource_exhausted   |
| `RUNTIME_ERROR`                                                                      | 未分类运行时错误                         | runtime_error        |
| `TOOL_ERROR`                                                                         | 外部函数返回错误                         | runtime_error        |
| `SNAPSHOT_VERSION_MISMATCH` / `SNAPSHOT_CAPABILITY_MISMATCH` / `SNAPSHOT_CORRUPT`    | snapshot 加载失败                        | validation_error     |
| `INVALID_SESSION_ID` / `INVALID_TOOL_NAME` / `INVALID_ARGUMENT`                      | 宿主请求形状错误                         | validation_error     |
| `METHOD_NOT_FOUND` / `SESSION_NOT_FOUND`                                             | JSON-RPC 方法或 session 不存在           | validation_error     |
| `IO_ERROR` / `SERIALIZE_ERROR`                                                       | 宿主 I/O 或序列化失败                    | runtime_error        |

错误码字符串不得重命名。新增错误码必须同步更新本文件和 [SKILL.md](SKILL.md)。

## 14. 安全测试矩阵

| 类别                | 用例                                       | 预期                                               |
| ------------------- | ------------------------------------------ | -------------------------------------------------- |
| 路径逃逸            | `read_text("/workspace/../etc/passwd")`    | `PERMISSION_DENIED`                                |
| 符号链接            | mount 内含指向 mount 外的 symlink          | `PERMISSION_DENIED`                                |
| 网络未授权          | `import urllib`、`socket`、`fetch(`        | `UNSUPPORTED_FEATURE` 或 `PERMISSION_DENIED`       |
| 子进程              | `os.system`、`subprocess`、`process.`      | `UNSUPPORTED_FEATURE`                              |
| 反射逃逸            | `().__class__.__bases__`                   | `UNSUPPORTED_FEATURE`                              |
| 危险 import         | 未声明或危险模块 import                    | `UNSUPPORTED_FEATURE`                              |
| 动态执行            | `eval`、`exec`、`Function`、dynamic import | `UNSUPPORTED_FEATURE`                              |
| 字符串误报          | 字符串字面量包含危险词                     | 不误报                                             |
| 资源耗尽            | 死循环、巨型 list、深递归                  | `TIMEOUT_*` / `MEMORY_EXCEEDED` / `STACK_OVERFLOW` |
| stdout 炸弹         | 无限 print                                 | 截断 + `STDOUT_EXCEEDED`                           |
| 外部调用风暴        | 循环调用 tool                              | `EXTERNAL_CALLS_EXCEEDED`                          |
| 工具错误传播        | tool 抛错                                  | `TOOL_ERROR` 保留 message/code                     |
| Snapshot 损坏       | 篡改 CBOR 字节                             | `SNAPSHOT_CORRUPT`                                 |
| Snapshot 版本不匹配 | 当前版本以外快照                           | `SNAPSHOT_VERSION_MISMATCH`                        |
| 输入注入            | `inputs` 含恶意 unicode/控制符             | 正常注入但不执行                                   |
| 密钥泄露            | tool 配置中的 secret                       | 不出现在 stdout / 日志 / snapshot                  |

## 15. 端到端验收

所有用例必须能在 CI 中以脚本方式跑通；CLI 与 daemon 输出必须是稳定 JSON。

1. CLI 单次执行：`langshell run -e 'result = sum(range(10))' --json`，断言 `status=ok`、`result=45`。
2. Session 状态复用：先写入 `cache`，再在同 session 中读取，断言结果可复用。
3. Validate 阻断未授权：`langshell validate -e 'open("/etc/passwd")' --json` 返回 `UNSUPPORTED_FEATURE`。
4. Async 外部函数 fan-out：注册 `fetch_json` 后并发调用，断言 external call 计数正确。
5. Snapshot / Restore：保存 CBOR snapshot 后恢复到新 session，断言状态可读取。
6. Daemon JSON-RPC：连续执行 create、run、snapshot、destroy，响应符合 schema。

## 16. 近期路线

- Durable snapshot store。
- Session fork / diff / reset / cancel。
- Tool schema 到 Python / TypeScript typed stubs 的生成。
- 更完整的错误修复建议和 span 映射。
- SQLite、object_store、search、embedding、issue tracker 等 plugin。
- Windows named pipe daemon transport。
- 可视化 trace / replay debugger。

## 17. 术语

- **Session**：状态、权限、限制、审计的最小单位。
- **Capability / Tool**：宿主注册到沙箱的具名函数。
- **Side Effect**：能力对外部世界的影响等级。
- **Snapshot**：可恢复的 session 序列化，不包含活跃句柄与密钥。
- **Mount**：宿主映射给沙箱的虚拟 POSIX 路径前缀。
- **Validate / Dry-run**：执行前静态检查，不产生副作用。
- **Approval Boundary**：副作用调用前的可中断点，可生成 snapshot 并暂停。