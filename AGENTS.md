# LangShell 产品需求文档

**版本**：1.2（2026-04-26）
**状态**：草案（开发指导版）
**项目定位**：专为 AI Agent 设计的状态持久、能力受控、安全沙箱代码执行层
**MVP 范围**：Python / Monty 优先；TypeScript / Deno 作为后续路线

> 本文档既是产品需求说明，也是 MVP 阶段的代码开发指导。第 1–12 章描述产品视角，第 13–18 章描述工程实现契约（仓库结构、数据结构、错误码、Snapshot、测试矩阵、e2e 用例、术语）。任何与契约章节不一致的实现都视为缺陷。

## 1. 使用者视角 Review

从 AI Agent 的使用体验看，LangShell 最有价值的地方不是“又多了一个工具”，而是把很多零碎工具调用变成一段可验证、可复用、可恢复的程序。Agent 可以用自己最擅长的代码表达循环、条件、并发、缓存、重试、数据变换和多步推理，而宿主仍然保持对外部世界的严格控制。

一个好用的 LangShell 必须同时满足三类使用者：

- **AI Agent**：希望直接写 Python 代码完成任务，保留中间状态，拿到清晰的结构化结果和可修复错误。
- **Agent 框架开发者**：希望用稳定协议嵌入执行能力，注册外部函数，控制权限、超时、资源和审计。
- **平台 / 安全负责人**：希望默认零权限，所有副作用都可追踪、可限制、可回放，且不会让 LLM 生成代码逃逸到宿主环境。

因此，LangShell 应该具备以下核心能力：

- **代码即接口**：Agent 输出 Python 代码即可执行复杂逻辑，不需要为每个微动作设计新的 tool schema。
- **状态持久**：同一 session 内的变量、函数、缓存、已导入安全模块、待恢复快照可持续复用。
- **能力发现**：Agent 能查询当前 session 被授予了哪些外部函数、文件挂载、网络策略和资源限制。
- **外部世界能力注入**：文件、HTTP、数据库、业务 API、邮件、GPU 等能力只能由宿主显式注册为函数。
- **安全沙箱**：默认无文件系统、无网络、无环境变量、无子进程、无任意 import；所有外部交互都走可审计能力。
- **异步编排**：支持 `async` / `await`，让 Agent 能做 fan-out / fan-in、并发请求和长耗时外部调用。
- **验证与修复**：支持 validate / dry-run，返回语法错误、类型错误、权限错误、超时错误和修复建议。
- **快照与恢复**：能在外部函数调用、超时、中断、人工审批点保存状态，并跨进程恢复。
- **可观测性**：记录代码、输入、输出、外部调用、资源消耗、快照 ID、trace ID，敏感信息按策略脱敏。
- **可嵌入接口**：提供 CLI、Daemon IPC、Rust SDK，后续提供 Python / TypeScript SDK。

产品边界同样重要：LangShell 不是完整 CPython、不是任意系统 shell、不是容器平台，也不应该鼓励 Agent 绕过宿主的权限模型。它的定位是“Agent 的安全程序化工具调用层”。

## 2. 背景与问题

传统 AI Agent 执行方式存在明显局限：

- **传统 tool-calling**：每个动作都需要预定义 schema；复杂逻辑需要多次往返，token 成本高，错误恢复困难。
- **普通 shell**：无天然状态、输出解析脆弱、权限过大、异步困难，且不适合直接运行不可信 LLM 代码。
- **容器沙箱**：安全边界强，但启动慢、资源重、集成复杂，对高频 Agent 内循环不够轻。

LangShell 的目标是提供一条更自然的路径：

> AI tokens -> Python code -> safe execution -> structured result -> resumable state

这让 Agent 可以把“如何完成任务”写成程序，同时让宿主决定“哪些外部能力可以被调用”。

## 3. 产品原则

- **Code is the interface**：对 Agent 来说，主要接口就是 Python 代码，而不是一串碎片化工具调用。
- **Session is the unit**：状态、权限、快照、资源限制、审计日志都以 session 为基本单位管理。
- **Capabilities over permissions**：默认没有权限；宿主只暴露明确命名、可描述、可限制的函数能力。
- **Every side effect is mediated**：文件、网络、数据库、邮件、业务 API 等副作用必须经过宿主函数。
- **Errors are for agents**：错误信息不仅给人看，也要让 Agent 能自动理解、修复和重试。
- **Small safe core, extensible edge**：核心运行时保持小而安全；外部能力通过 registry / plugin 扩展。

## 4. 目标与非目标

### 4.1 目标

- 让 Agent 一次性表达多步逻辑、数据处理、重试、并行和缓存。
- 提供状态持久的 Python 执行环境，减少重复上下文和重复 tool call。
- 通过 Monty 提供微秒级启动、资源可控、无 CPython 依赖的安全 Python 子集。
- 通过宿主注册函数安全访问外部系统。
- 通过结构化 JSON 输出，让 Agent 和框架可以稳定解析结果。
- 通过 CLI、IPC 和 SDK 支持不同集成场景。

### 4.2 非目标

- 不做完整 CPython 替代品。
- 不支持任意第三方 Python 包作为沙箱内依赖。
- 不提供默认宿主文件系统、网络、环境变量或子进程访问。
- 不把 LangShell 设计成通用 OS shell。
- 不用 LangShell 绕过现有 Agent 平台的安全审批和审计机制。

## 5. 核心功能需求

### 5.1 Python 执行核心

- 使用 Pydantic Monty 作为 MVP Python 执行引擎。
- 支持 Agent 常用 Python 子集：变量、函数、控制流、列表 / 字典 / 集合、推导式、异常处理、类型提示、`async` / `await`。
- 支持一小部分安全标准库：`typing`、`asyncio`、`json`、`re`、`datetime`、`dataclasses` 等，以 Monty 实际支持为准。
- 支持 top-level await，便于 Agent 直接编写异步代码。
- 支持 stdout / stderr 捕获。
- 支持 validate 模式：只做语法检查、类型检查、权限检查和外部函数存在性检查，不执行副作用。
- 明确暴露当前 Python 子集限制。MVP 不承诺完整标准库、第三方包、任意 class 语义和 `match` 语句，除非 Monty 已稳定支持。

### 5.2 状态与 Session

- `session` 是执行、权限、状态、资源限制和审计的基本单位。
- 同一 session 内持久保留：变量、函数、安全模块导入、缓存数据、外部函数 stubs、快照元数据。
- 支持 session 生命周期：create、run、validate、snapshot、restore、fork、reset、cancel、destroy、list。
- 支持 session TTL、最大内存、最大执行时间、最大输出大小、最大外部调用次数等限制。
- 支持命名 session，便于 Agent 在多轮对话中复用上下文。
- 支持快照版本化，避免运行时升级后无法恢复旧状态时静默失败。

### 5.3 外部函数 Registry

宿主可以把安全能力注册为沙箱内可调用函数。每个函数需要包含：

- `name`：沙箱内函数名，例如 `fetch_json`、`read_text`、`write_text`、`query_db`。
- `description`：给 Agent 看的简短说明。
- `input_schema` / `type_stub`：参数类型、默认值、约束。
- `output_schema`：返回值结构说明。
- `side_effect`：`none`、`read`、`write`、`network`、`database`、`external_system`。
- `limits`：单次调用超时、最大请求体、最大响应体、并发数、速率限制。
- `approval_policy`：是否需要人工或上层策略审批。
- `idempotency`：是否支持 idempotency key，便于安全重试。

外部函数必须支持 sync 和 async 两种实现，LangShell 负责在 Python 代码中以普通函数或 `await` 调用的形式暴露。

### 5.4 能力发现

Agent 需要先知道能调用什么，而不是猜函数名。LangShell 应提供内置发现函数：

```python
tools = list_tools()
info = describe_tool("fetch_json")
policy = current_policy()
```

返回信息应包含函数描述、参数、返回值、示例、权限、资源限制和副作用等级。

### 5.5 沙箱安全模型

- 默认禁止宿主文件系统、环境变量、网络、子进程、动态库、危险 import、反射逃逸。
- 文件访问只能通过虚拟挂载或注册函数完成。
- 沙箱内路径统一使用 POSIX 风格，例如 `/workspace/input.json`，由宿主映射到平台原生路径。
- 路径解析必须防止 `..`、符号链接逃逸、空字节、大小写规避和挂载边界绕过。
- 网络访问必须通过注册函数，并支持域名 allowlist、协议限制、代理、超时、重试和响应大小限制。
- 外部函数是主要风险边界，需要记录调用参数、结果摘要、耗时、错误和审批结果。
- 快照反序列化必须只接受可信来源，避免把 untrusted snapshot 当作安全输入。

### 5.6 内置能力分层

MVP 应内置少量高价值、安全可控能力：

- 文件：`read_text`、`write_text`、`list_dir`，仅限授权虚拟目录。
- HTTP：`fetch_text`、`fetch_json`，仅限 allowlist 域名和大小限制。
- JSON：优先使用沙箱内 `json` 标准库；大对象可由宿主函数辅助处理。
- 系统信息：只读、脱敏、最小化，例如当前时间、平台标签、可用工具列表。
- 调试：`list_tools`、`describe_tool`、`current_policy`。

Post-MVP 以 plugin 形式提供：

- CSV / Parquet / Arrow 大数据处理。
- SQLite / PostgreSQL 查询。
- Vector DB / search / embedding 相关函数。
- 邮件、Slack、Issue tracker、CRM 等业务系统连接器。
- 人工审批与事务型副作用函数。

## 6. 使用接口设计

### 6.1 Agent-facing Python 接口

Agent 在使用 LangShell 时只需要输出 Python 代码。推荐风格：

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

设计要求：

- 支持 top-level await。
- 支持将最后表达式、`result` 变量或 stdout 作为结果来源；具体优先级由协议定义。
- 鼓励 Agent 返回小而清晰的 JSON 对象，而不是长文本。
- 鼓励 Agent 在全局变量中缓存可复用数据，例如解析后的配置、已拉取数据、编译后的正则。
- 对副作用函数，推荐提供 idempotency key，避免重试时重复写入。

### 6.2 CLI

CLI 面向开发者调试、脚本集成和本地使用。

```bash
langshell run -e 'x = 1 + 1; print(x)' --json
langshell run -f script.py --session-id agent-123 --timeout 5s
langshell validate -f script.py --tools tools.json
langshell repl --session-id agent-123
langshell daemon --listen unix:///tmp/langshell.sock
langshell session list
langshell session snapshot agent-123 --out snapshot.bin
langshell session restore --from snapshot.bin --session-id restored
langshell tools list --session-id agent-123
langshell tools describe fetch_json --session-id agent-123
```

CLI 默认输出 JSON；可选 human-readable 模式用于调试。

### 6.3 Daemon / IPC 协议

Daemon 适合长期运行和多 Agent 共享。协议建议采用 JSON-RPC 2.0，传输支持 stdio、Unix socket、Windows named pipe。

核心方法：

- `session.create`
- `session.run`
- `session.validate`
- `session.cancel`
- `session.snapshot`
- `session.restore`
- `session.fork`
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
    "code": "data = await fetch_json(url)\nresult = {'count': len(data)}\nprint(result)",
    "inputs": {"url": "https://api.example.com/items"},
    "timeout_ms": 5000,
    "limits": {
      "memory_mb": 64,
      "cpu_ms": 2000,
      "max_stdout_bytes": 65536,
      "max_external_calls": 20
    },
    "return_snapshot": true
  }
}
```

响应示例：

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "result": {
    "status": "ok",
    "result": {"count": 42},
    "stdout": "{'count': 42}\n",
    "stderr": "",
    "diagnostics": [],
    "external_calls": [
      {
        "name": "fetch_json",
        "side_effect": "network",
        "duration_ms": 128,
        "status": "ok"
      }
    ],
    "snapshot_id": "snap_01J...",
    "metrics": {
      "duration_ms": 141,
      "memory_peak_bytes": 1048576,
      "instructions": 9210
    }
  }
}
```

错误响应需要稳定、可机器解析：

```json
{
  "status": "error",
  "error": {
    "code": "UNKNOWN_TOOL",
    "message": "Function fetch_url is not registered in this session.",
    "hint": "Call list_tools() or use fetch_json if available.",
    "span": {"line": 1, "column": 14}
  },
  "stdout": "",
  "stderr": "",
  "snapshot_id": "snap_before_error"
}
```

### 6.4 Rust SDK

Rust SDK 是宿主集成的核心接口：

```rust
let runtime = LangShell::builder()
    .runtime(langshell_monty::MontyRuntime::new)
    .memory_limit_mb(64)
    .timeout_ms(5_000)
    .cancel_token(cancel_token)
    .mount_readonly("/workspace", host_workspace)
    .envs(vec![("API_KEY", "secret")])
    .register_async("fetch_json", fetch_json_fn)
    .register_async("write_text", write_text_fn)
    .build()?;

let result = runtime
    .session("agent-123")
    .run(code)
    .input(json!({"url": url}))
    .await?;
```

SDK 需要强制开发者显式声明 runtime backend、能力、限制和副作用等级，避免无意暴露宿主环境。

## 7. 状态码与结果语义

标准状态码：

- `ok`：执行成功。
- `validation_error`：语法、类型、权限或工具存在性检查失败。
- `runtime_error`：代码运行时错误。
- `timeout`：超过执行或外部调用时间限制。
- `cancelled`：被用户或宿主取消。
- `resource_exhausted`：内存、栈深、输出大小、外部调用次数等超限。
- `permission_denied`：请求了未授权能力。
- `waiting_for_approval`：外部副作用需要人工或策略审批。
- `interrupted`：执行在可恢复点中断，并返回 snapshot。

结果优先级建议：

1. 显式 `return` 值或协议指定变量，例如 `result`。
2. 最后表达式值。
3. stdout。
4. 仅状态和诊断。

MVP 可以先支持 `result` 变量 + stdout，后续再支持最后表达式捕获。

## 8. 非功能需求

- **性能**：Monty 引擎启动目标为微秒级；daemon 模式下单次小代码执行 P50 < 20ms，P95 < 100ms，不包含外部网络耗时。
- **资源控制**：支持 CPU 时间、墙钟时间、内存、分配次数、栈深、输出大小、外部调用次数限制。
- **安全性**：默认零权限；所有逃逸面必须有安全测试；外部函数 registry 是主要审计边界。
- **可靠性**：daemon 崩溃后可通过最近 snapshot 恢复；错误响应稳定，不随内部实现大幅变化。
- **跨平台**：单 binary 支持 macOS、Linux、Windows；虚拟路径语义保持一致。
- **可观测性**：集成 tracing，记录 trace ID、session ID、代码 hash、外部调用、资源指标和错误诊断。
- **隐私与密钥**：默认不把密钥注入沙箱；日志按策略脱敏；外部函数负责最小权限访问。
- **兼容性**：snapshot、协议和工具描述需要版本字段，支持向后兼容或明确迁移错误。

## 9. 技术架构

- **执行引擎**：Pydantic Monty（Rust 实现的安全 Python 子集 VM）。
- **宿主层**：Rust，负责 CLI、daemon、IPC、session、policy、registry、snapshot、tracing。
- **外部能力层**：Rust async/sync 函数 registry；后续支持 plugin。
- **通信层**：JSON-RPC over stdio / Unix socket / Windows named pipe。
- **存储层**：MVP 支持内存 session 和文件 snapshot；后续支持 SQLite / object_store。
- **未来执行引擎**：TypeScript / Deno，以独立 language backend 接入同一 session / capability / policy 模型。

## 10. 路线图

### MVP

- Monty 集成，支持 Python 子集执行。
- session 内状态持久。
- `result` / stdout / stderr / diagnostics JSON 输出。
- validate 模式。
- 超时、取消、内存和输出大小限制。
- 外部函数 registry，支持 sync / async。
- 内置 `list_tools`、`describe_tool`、`current_policy`。
- 安全虚拟文件能力：`read_text`、`write_text`、`list_dir`。
- 安全 HTTP 能力：`fetch_text`、`fetch_json`。
- CLI：`run`、`validate`、`repl`、`daemon`、`session`、`tools`。
- IPC JSON-RPC：create / run / validate / cancel / snapshot / restore。
- 基础 tracing 和审计日志。

### V1

- durable snapshot store。
- typed stubs 自动生成与工具说明注入。
- session fork / diff / reset。
- 更完整的错误修复建议。
- SQLite / object_store 插件。
- SDK 文档与示例项目。

### VNext

- TypeScript / Deno backend。
- 多租户 daemon。
- 分布式 session 和远程执行。
- 事务型副作用、补偿操作和 saga 风格工作流。
- 可视化 trace / replay debugger。

## 11. 验收标准

MVP 成功标准：

- Agent 能通过 `langshell run` 执行 Python 代码，并收到稳定 JSON 结果。
- 同一 session 中前一次定义的变量和函数能在后一次执行中复用。
- 未授权文件、网络、环境变量、子进程访问被阻止，并返回 `permission_denied` 或 `validation_error`。
- 注册的 async 外部函数能从 Python 中 `await` 调用，并在审计日志中出现。
- validate 模式能发现语法错误、未知函数、类型不匹配和未授权能力，且不产生副作用。
- 超时、取消、内存超限和输出超限都能稳定返回机器可解析错误。
- snapshot 能保存并恢复至少一次外部函数调用边界或 session 状态。
- CLI、daemon IPC、Rust SDK 至少各有一个端到端示例。
- 安全测试覆盖路径逃逸、网络未授权、危险 import、资源耗尽、外部函数错误传播。

最终产品成功定义：AI 能自然地说“use LangShell”，用一段安全 Python 完成复杂数据处理和外部能力编排；开发者无需为每个微动作预写新 tool schema，只需提供可复用、可审计、可限制的能力函数。

## 12. 风险与待定问题

- Monty 当前是实验性 Python 子集，类、完整标准库、第三方库和部分语法能力不能按 CPython 预期承诺。
- “持久 async task”需要谨慎设计，避免后台任务在 Agent 不可见时继续产生副作用。
- 快照跨版本兼容性和安全加载策略需要尽早确定。
- 外部函数 registry 是最大安全边界，需要默认安全的 SDK 设计和示例。
- 结果捕获语义需要尽早稳定：`result` 变量、最后表达式、stdout 三者如何排序。

---

## 13. 仓库与模块结构（开发契约）

仓库根目录使用 Cargo workspace，并以 git submodule 引入上游执行引擎：

```
langshell/
├── monty/                  # submodule: pydantic/monty（Rust 实现的 Python 子集 VM）
├── deno/                   # submodule: denoland/deno（VNext 的 TS backend）
├── crates/
│   ├── langshell-core/     # session、policy、registry、snapshot、diagnostics 抽象（无引擎依赖）
│   ├── langshell-monty/    # core 接口的 Monty 实现（MVP 默认 backend）
│   ├── langshell-deno/     # core 接口的 Deno / TypeScript 实现
│   ├── langshell-tools/    # 内置能力：read_text/write_text/list_dir/fetch_text/fetch_json
│   ├── langshell-cli/      # `langshell` 二进制：run/validate/repl/daemon/session/tools
│   └── langshell/          # 公共 Rust SDK（builder、runtime、register_async、run）
├── docs/
└── examples/               # CLI / daemon / SDK 各一个 e2e 示例
```

模块边界规则：

- `langshell-core` 不得依赖具体执行引擎，只定义 trait 与数据类型，包括 `LanguageRuntime` 后端抽象。
- `langshell-monty` 是唯一可以依赖 `monty/` 的 crate；任何 Monty API 变化都封装在此。
- `langshell` 不得依赖 `langshell-monty` 或 `langshell-deno`；宿主必须通过 `LanguageRuntime` trait 显式组装所需后端。
- `langshell-daemon` 只做协议序列化/反序列化与传输，不持有 session 状态。
- `langshell-tools` 中的每个能力函数是一个独立模块，便于单独启用/禁用与单独审计。
- CLI 作为宿主可依赖并组装 `langshell-monty` / `langshell-deno`；SDK 仅依赖 `langshell-core` + `langshell-tools`，二者不得互相依赖。

## 14. 关键数据结构（Rust 契约）

以下类型为对外稳定契约，字段命名与 JSON-RPC 序列化保持一致（snake_case）。所有面向 Agent / 客户端的枚举均使用字符串标签序列化（`#[serde(rename_all = "snake_case")]`）。

```rust
// crates/langshell-core/src/session.rs
pub struct SessionId(pub String);             // 用户可指定，必须匹配 ^[a-zA-Z0-9_\-]{1,64}$

pub struct SessionLimits {
    pub memory_mb: u32,                        // 默认 64
    pub cpu_ms: u32,                           // 默认 2_000
    pub wall_ms: u32,                          // 默认 5_000
    pub max_stdout_bytes: u32,                 // 默认 65_536
    pub max_external_calls: u32,               // 默认 32
    pub max_stack_depth: u16,                  // 默认 256
}

pub struct SessionMeta {
    pub id: SessionId,
    pub created_at: u64,                       // unix ms
    pub last_used_at: u64,
    pub language: Language,                    // Python | TypeScript(VNext)
    pub limits: SessionLimits,
    pub snapshot_version: u32,                 // 见 §16
}

// crates/langshell-core/src/capability.rs
pub enum SideEffect { None, Read, Write, Network, Database, ExternalSystem }

pub struct Capability {
    pub name: String,                          // 沙箱内函数名，必须是合法 Python 标识符
    pub description: String,
    pub input_schema: serde_json::Value,       // JSON Schema draft-2020-12
    pub output_schema: serde_json::Value,
    pub side_effect: SideEffect,
    pub limits: CapabilityLimits,              // timeout_ms / max_request_bytes / max_response_bytes / concurrency / rate_per_min
    pub approval_policy: ApprovalPolicy,       // None | Auto(rule) | Manual
    pub idempotent: bool,
}

// crates/langshell-core/src/run.rs
pub struct RunRequest {
    pub session_id: SessionId,
    pub language: Language,
    pub code: String,
    pub inputs: serde_json::Map<String, serde_json::Value>,  // 注入为顶层只读变量
    pub timeout_ms: Option<u32>,
    pub limits: Option<SessionLimits>,         // 仅本次调用覆盖
    pub return_snapshot: bool,
    pub validate_only: bool,
}

pub struct RunResult {
    pub status: RunStatus,                     // 见 §7 状态码
    pub result: Option<serde_json::Value>,     // 按 §15 优先级捕获
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Vec<Diagnostic>,
    pub external_calls: Vec<ExternalCallRecord>,
    pub snapshot_id: Option<String>,
    pub metrics: Metrics,                      // duration_ms / memory_peak_bytes / instructions / external_calls_count
    pub error: Option<ErrorObject>,            // 仅当 status != ok
}

pub struct Diagnostic {
    pub severity: Severity,                    // error | warning | info
    pub code: String,                          // 与 §17 错误码对齐
    pub message: String,
    pub hint: Option<String>,
    pub span: Option<Span>,                    // line/column/end_line/end_column 1-based
}

pub struct ExternalCallRecord {
    pub name: String,
    pub side_effect: SideEffect,
    pub duration_ms: u32,
    pub status: CallStatus,                    // ok | error | timeout | denied | approval_pending
    pub request_digest: String,                // sha256 of canonical args，避免泄露原始内容
    pub response_digest: Option<String>,
    pub error: Option<ErrorObject>,
}
```

实现要求：

- 所有公共类型必须实现 `Serialize + Deserialize + Clone + Debug`。
- 不允许在公共结构体上派生 `Default` 除非所有字段类型都实现 `Default`；否则手写 `Default`。
- 任何新增字段必须可选或有默认值，且 bump `snapshot_version` 不会破坏旧 snapshot 的反序列化。

## 15. 结果捕获优先级（MVP 锁定）

MVP 锁定如下顺序，避免歧义：

1. 若代码顶层定义了名为 `result` 的变量（在执行结束时存在于全局作用域），其值作为 `RunResult.result`。
2. 否则若代码最后一条语句是表达式，则使用其求值结果（仅当 `validate_only = false` 且引擎已支持表达式捕获）。
3. 否则 `RunResult.result = null`，调用方应回退使用 stdout。

约束：

- 若 `result` 不是 JSON 可序列化值（如包含函数、生成器、不可序列化对象），返回 `validation_error` 子类 `RESULT_NOT_SERIALIZABLE`，并把对象类型名写入 hint。
- stdout / stderr 各自独立计入 `max_stdout_bytes`；超限时截断并附 `OUTPUT_TRUNCATED` 警告。

## 16. Snapshot 格式与版本化

- 容器：`langshell-snapshot/v{N}` 为前缀的二进制，使用 `bincode` 编码，外层加 8 字节魔数 `LSNAPSHT` + 4 字节大端 `version`。
- 必含字段：`session_meta`、`globals`（命名值 + 类型标签）、`registered_capabilities`（仅名字 + 哈希，不含实现）、`pending_external_call`（如在 awaiting_approval/interrupted 状态）。
- 加载策略：
  - 仅接受同一 `snapshot_version` 或 `core` 内白名单升级器（`v1 -> v2` 等）。
  - 加载前校验 capabilities 的哈希集是 host 当前注册集的子集；不满足则返回 `SNAPSHOT_CAPABILITY_MISMATCH`。
  - 反序列化必须在受限读取上下文中完成（限制递归深度、最大字节数 64 MiB 默认）。
- 不在 snapshot 中持久化：实时 socket / 文件句柄 / 已完成的 stdout 缓冲 / 密钥。

## 17. 错误码与诊断（稳定契约）

错误对象统一形状：

```json
{ "code": "UNKNOWN_TOOL", "message": "...", "hint": "...", "span": { "line": 1, "column": 14 } }
```

MVP 必须实现的错误码（与 `RunStatus` 的对应在括号内）：

| Code                                                                                 | 触发条件                               | Status               |
| ------------------------------------------------------------------------------------ | -------------------------------------- | -------------------- |
| `SYNTAX_ERROR`                                                                       | Python 解析失败                        | validation_error     |
| `TYPE_ERROR`                                                                         | validate 类型不匹配                    | validation_error     |
| `UNKNOWN_TOOL`                                                                       | 调用未注册函数                         | validation_error     |
| `UNSUPPORTED_FEATURE`                                                                | 使用 Monty 不支持的语法/标准库         | validation_error     |
| `RESULT_NOT_SERIALIZABLE`                                                            | `result` 不是 JSON 可序列化            | validation_error     |
| `PERMISSION_DENIED`                                                                  | 访问未授权能力或路径                   | permission_denied    |
| `WAITING_FOR_APPROVAL`                                                               | 副作用需人工/策略审批                  | waiting_for_approval |
| `TIMEOUT_WALL` / `TIMEOUT_CPU` / `TIMEOUT_TOOL`                                      | 各类超时                               | timeout              |
| `CANCELLED`                                                                          | 主动取消                               | cancelled            |
| `MEMORY_EXCEEDED` / `STDOUT_EXCEEDED` / `EXTERNAL_CALLS_EXCEEDED` / `STACK_OVERFLOW` | 资源超限                               | resource_exhausted   |
| `RUNTIME_ERROR`                                                                      | 未分类的运行时异常                     | runtime_error        |
| `TOOL_ERROR`                                                                         | 外部函数自身返回错误                   | runtime_error        |
| `SNAPSHOT_VERSION_MISMATCH` / `SNAPSHOT_CAPABILITY_MISMATCH` / `SNAPSHOT_CORRUPT`    | snapshot 加载失败                      | validation_error     |
| `INTERRUPTED`                                                                        | 在审批/外部调用边界中断并产生 snapshot | interrupted          |

实现要求：错误码字符串永不重命名；新增码必须更新 §17 与 SKILL 文档。

MVP 实现辅助错误码（主要用于 CLI / SDK / JSON-RPC 宿主层输入与 I/O）：

| Code                 | 触发条件                                   | Status           |
| -------------------- | ------------------------------------------ | ---------------- |
| `INVALID_SESSION_ID` | session id 不符合 `^[a-zA-Z0-9_\-]{1,64}$` | validation_error |
| `INVALID_TOOL_NAME`  | capability 名称不是合法 Python 标识符      | validation_error |
| `INVALID_ARGUMENT`   | CLI / JSON-RPC 参数缺失或形状错误          | validation_error |
| `METHOD_NOT_FOUND`   | JSON-RPC 方法未实现                        | validation_error |
| `SESSION_NOT_FOUND`  | 请求 snapshot 一个不存在的 session         | validation_error |
| `IO_ERROR`           | CLI 文件、socket 或 session store I/O 失败 | runtime_error    |
| `SERIALIZE_ERROR`    | 宿主层 JSON 输出序列化失败                 | runtime_error    |

## 18. 安全测试矩阵（MVP 必过）

每条用例对应 `crates/langshell-core/tests/` 或 `examples/` 中的一个测试。

| 类别            | 用例                                    | 预期                                               |
| --------------- | --------------------------------------- | -------------------------------------------------- |
| 路径逃逸        | `read_text("/workspace/../etc/passwd")` | `PERMISSION_DENIED`                                |
| 符号链接        | mount 内含指向 mount 外的 symlink       | `PERMISSION_DENIED`                                |
| 网络未授权      | `import urllib`、`socket`               | `UNSUPPORTED_FEATURE` 或 `PERMISSION_DENIED`       |
| 子进程          | `os.system`、`subprocess`               | `UNSUPPORTED_FEATURE`                              |
| 反射逃逸        | 通过 `__class__.__bases__` 链找 `os`    | `UNSUPPORTED_FEATURE` 或不可达                     |
| 危险 import     | 任意未声明 import                       | `UNSUPPORTED_FEATURE`                              |
| 资源耗尽        | 死循环、巨型 list、深递归               | `TIMEOUT_*` / `MEMORY_EXCEEDED` / `STACK_OVERFLOW` |
| stdout 炸弹     | 无限 print                              | 截断 + `STDOUT_EXCEEDED`                           |
| 外部调用风暴    | 循环 `await fetch_json`                 | `EXTERNAL_CALLS_EXCEEDED`                          |
| 工具错误传播    | tool 抛错                               | `TOOL_ERROR` 含原 message/code                     |
| Snapshot 注入   | 篡改 snapshot 字节                      | `SNAPSHOT_CORRUPT`                                 |
| Snapshot 跨版本 | v1 加载到 v2 无升级器                   | `SNAPSHOT_VERSION_MISMATCH`                        |
| 输入注入        | `inputs` 含恶意 unicode/控制符          | 正常注入但不执行                                   |
| 密钥泄露        | tool 配置中的 secret                    | 不出现在 stdout / 日志 / snapshot                  |

每个用例必须既覆盖 `validate_only=true` 也覆盖 `validate_only=false`（除非语义无关）。

## 19. MVP 端到端验收用例

所有用例必须能在 CI 中以脚本方式跑通；CLI 与 daemon 输出必须是稳定 JSON。

1. **CLI 单次执行**

   ```bash
   langshell run -e 'result = sum(range(10))' --json
   ```

   断言：`status=ok`，`result=45`，`metrics.duration_ms < 50`。

2. **Session 状态复用**

   ```bash
   langshell run -e 'cache = {"k": 1}' --session-id s1 --json
   langshell run -e 'result = cache["k"] + 1' --session-id s1 --json
   ```

   断言：第二次 `result=2`。

3. **Validate 阻断未授权**

   ```bash
   langshell validate -e 'open("/etc/passwd")' --json
   ```

   断言：`status=validation_error`，`error.code=UNSUPPORTED_FEATURE`，无任何文件被访问。

4. **Async 外部函数 fan-out**（SDK 注册 `fetch_json`，allowlist `api.example.com`）

   ```python
   import asyncio
   data = await asyncio.gather(*(fetch_json(f"https://api.example.com/i/{i}") for i in range(3)))
   result = {"n": len(data)}
   ```

   断言：`external_calls` 长度为 3，`metrics.external_calls_count=3`。

5. **Snapshot / Restore**

   ```bash
   langshell run -e 'state = {"step": 1}' --session-id s2 --json
   langshell session snapshot s2 --out /tmp/s2.snap
   langshell session restore --from /tmp/s2.snap --session-id s2r
   langshell run -e 'result = state["step"]' --session-id s2r --json
   ```

   断言：恢复后 `result=1`。

6. **Daemon JSON-RPC**：通过 Unix socket 连续发送 `session.create` → `session.run` → `session.snapshot` → `session.destroy`，每步响应符合 §6.3 schema。

每个用例都对应 `examples/` 中可独立运行的脚本，README 链接齐全。

## 20. 术语表

- **Session**：状态、权限、限制、审计的最小单位，由 `SessionId` 唯一标识。
- **Capability / Tool**：宿主注册到沙箱的具名函数；Agent 视角是工具，宿主视角是能力。
- **Side Effect**：能力对外部世界的影响等级，决定审批和审计策略。
- **Snapshot**：可恢复的 session 序列化；不包含活跃句柄与密钥。
- **Mount**：宿主映射给沙箱的虚拟 POSIX 路径前缀。
- **Validate / Dry-run**：执行前的静态检查，不产生副作用。
- **Approval Boundary**：副作用调用前的可中断点，可在此生成 snapshot 并暂停。
