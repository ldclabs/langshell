# LangShell

> 面向 AI Agent 的状态持久、能力受控、安全沙箱代码执行层。

**[English](./README.md) | [中文](./README_CN.md)**

LangShell 让宿主可以在持久 session 中运行 Agent 生成的 Python 或 TypeScript 代码，同时把文件、网络、数据库和业务系统等外部交互全部收束到宿主显式注册的 capability 函数上。

项目处于活跃开发阶段。在稳定版本发布前，本地 session 快照和内部数据格式都可以直接变更，不提供旧数据迁移兼容。当前开发基线使用 CBOR 快照、基于 AST 的静态校验，以及带 schema 的 capability 描述。

## 当前状态

- `langshell-core` 定义 session、capability、diagnostic、metric、error、snapshot 和 runtime backend 的共享契约。
- `langshell-monty` 通过 Monty 执行 Python 子集代码，支持持久 session、top-level await、AST 校验、结果捕获、外部调用审计和 CBOR 快照。
- `langshell-deno` 通过 Deno/V8 执行 TypeScript，实现同一个 runtime trait，支持持久 globals、AST 校验、带类型标签的 CBOR 快照和 async capability 调度。
- `langshell-tools` 提供 discovery 工具，以及宿主可配置的文件和 HTTP capability helper。
- `langshell` 提供后端无关的 Rust SDK builder，用于配置限制、挂载、allowlist、sync/async capability 和 runtime。
- `langshell-cli` 提供 `run`、`validate`、`repl`、`daemon`、`session`、`tools` 命令，输出稳定 JSON。
- 端到端脚本和 SDK 覆盖位于 [examples/README.md](./examples/README.md)、后端 crate 测试和 SDK 测试中。

[AGENTS.md](AGENTS.md) 是产品与工程契约来源。[SKILL.md](SKILL.md) 描述 AI Agent 如何安全使用 LangShell。

## 为什么是 LangShell

传统 Agent 执行路径常落在两个不理想的端点：

- Tool calling 足够安全，但过于碎片化。复杂任务会变成大量往返、重复上下文、脆弱恢复和不断扩张的 schema。
- 普通 shell 足够表达力强，但权限过大。它拥有广泛的环境访问、弱结构化输出、脆弱解析，也没有天然 capability 边界。

LangShell 试图提供中间层：

```text
AI tokens -> sandboxed code -> mediated capabilities -> structured result -> resumable state
```

它同时服务三类角色：

- AI Agent 用代码表达循环、分支、缓存、重试、并发和数据变换。
- Agent 框架开发者嵌入稳定 runtime、注册工具、控制限制并收集审计记录。
- 平台和安全负责人保持默认闭合策略，让每个副作用都经过命名 capability。

## 核心模型

- **代码即接口**：Agent 用代码表达多步逻辑，而不是拆成大量微小 tool call。
- **Session 是状态单元**：变量、函数、globals、capability、限制和快照都属于 session。
- **Capability 替代环境权限**：文件系统、网络、数据库和业务 API 只能通过注册函数访问。
- **执行前校验**：Python 和 TypeScript 都通过 AST 检测危险 import、危险全局、反射逃逸模式和未知 capability-like 调用。
- **开发期版本化快照**：当前快照是 CBOR v2。旧本地快照不会被迁移。

## 已实现能力

- Monty 后端的状态持久 Python 子集执行。
- Deno/V8 后端的状态持久 TypeScript 执行。
- Top-level await 与 async capability 调用。
- Validate / dry-run，在副作用发生前捕获语法、权限、特性和工具可用性问题。
- `list_tools`、`describe_tool`、`current_policy` 能力发现。
- 结构化 `RunResult`，包含 `result`、stdout、stderr、diagnostic、external call、metric 和稳定错误码。
- 结果捕获优先级：全局 `result`，其次是后端支持的最后表达式，再回退 stdout。
- 墙钟超时、输出大小、内存、栈深和外部调用次数限制。
- 用于 session restore 的 CBOR 快照。Deno 快照对 `bigint`、`Uint8Array`、`Map`、`Set` 和 `Date` 做类型标签保存。
- 基于 Unix socket 的 JSON-RPC daemon，用于 session、run、snapshot、restore 和 tools 操作。

## 当前约束

- Monty 是 Python 子集，不是 CPython。未支持的标准库、第三方包、子进程、raw socket 和反射逃逸会被阻断或不可用。
- TypeScript 后端已经可用，但 Deno/V8 生命周期封装在 `LanguageRuntime` trait 和专用 worker 中。
- 文件工具只有在宿主配置授权虚拟挂载后才可用。
- 内置 HTTP helper 会执行 allowlist 和 schema 检查，但默认构建不包含真实网络传输；宿主需要注册自己的 `fetch_text` 或 `fetch_json` capability。
- CLI daemon 当前支持 `unix://` listener。
- 开发期数据可丢弃：本地 session 文件和快照可能随代码变更失效。

## 架构

```text
Agent / Host App
    |
    +-- CLI
    +-- JSON-RPC Daemon
    +-- Rust SDK
            |
            v
      langshell SDK
            |
     +------+----------------+
     |                       |
     v                       v
langshell-tools        langshell-core
                              |
                              v
                       LanguageRuntime trait
                              |
              +---------------+---------------+
              |                               |
              v                               v
       langshell-monty                 langshell-deno
              |                               |
              v                               v
           Monty VM                         Deno/V8
```

## Crates

| Crate             | 职责                                                                                 |
| ----------------- | ------------------------------------------------------------------------------------ |
| `langshell-core`  | session、run、capability、diagnostic、metric、snapshot 和 runtime trait 的共享契约。 |
| `langshell-monty` | 基于 Monty 的 Python runtime 实现。                                                  |
| `langshell-deno`  | 基于 Deno/V8 的 TypeScript runtime 实现。                                            |
| `langshell-tools` | Discovery、文件和 HTTP capability helper。                                           |
| `langshell`       | 用于组合 runtime 与注册 capability 的公共 Rust SDK。                                 |
| `langshell-cli`   | CLI 二进制与 JSON-RPC daemon。                                                       |

## 仓库结构

```text
langshell/
├── crates/
│   ├── langshell/
│   ├── langshell-cli/
│   ├── langshell-core/
│   ├── langshell-deno/
│   ├── langshell-monty/
│   └── langshell-tools/
├── docs/
├── examples/
├── AGENTS.md
├── SKILL.md
└── README.md
```

## Python 示例

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

## CLI 示例

```bash
cargo run -q -p langshell-cli --bin langshell -- run -e 'result = sum(range(10))' --json
cargo run -q -p langshell-cli --bin langshell -- validate -e 'open("/etc/passwd")' --json
cargo run -q -p langshell-cli --bin langshell -- session list
cargo run -q -p langshell-cli --bin langshell -- daemon --listen unix:///tmp/langshell.sock
```

## Rust SDK 示例

```rust
use langshell::{LangShell, SideEffect};
use langshell_monty::MontyRuntime;
use serde_json::{Value, json};

let shell = LangShell::builder()
    .runtime(MontyRuntime::new)
    .register_async(
        "fetch_json",
        "Fetch JSON from an approved source.",
        SideEffect::Network,
        json!({"type": "array", "prefixItems": [{"type": "string"}], "minItems": 1, "maxItems": 1}),
        json!({"type": "object"}),
        |ctx| async move {
            let url = ctx.args.first().and_then(Value::as_str).unwrap_or_default();
            Ok(json!({"url": url, "ok": true}))
        },
    )?
    .build()?;
```

## JSON-RPC 请求形状

Daemon 使用 Unix socket 上的 line-delimited JSON-RPC 2.0。

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

## 开发

### 环境要求

- 支持 Edition 2024 的 Rust stable toolchain。
- Git submodule。
- macOS、Linux 或 Windows。

### 拉取仓库

```bash
git clone --recurse-submodules <repo-url>
cd langshell
```

如果仓库不是用 submodule 模式克隆：

```bash
git submodule update --init --recursive
```

### 检查

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

### 端到端脚本

```bash
bash examples/cli_single.sh
bash examples/session_reuse.sh
bash examples/validate_denied.sh
bash examples/snapshot_restore.sh
cargo run -q -p langshell-monty --example sdk_async_fanout
```

CLI 会在设置 `LANGSHELL_SESSION_DIR` 时把开发 session 快照写入该目录，否则使用平台临时目录。这些文件不是稳定存储。

## 近期工作

- Durable snapshot store。
- Session fork / diff / reset。
- 更丰富的生成 stubs 与工具描述。
- 带真实 transport 的 HTTP helper 和更多 capability 模块。
- 更完整的安全测试：路径逃逸、工具调用风暴、snapshot 损坏和资源耗尽。
- Windows named pipe daemon transport。

## 文档入口

- [AGENTS.md](AGENTS.md)：产品方向、runtime 契约、数据结构、snapshot 格式、错误码和测试矩阵。
- [SKILL.md](SKILL.md)：Agent 侧安全使用指南。
- [examples/README.md](./examples/README.md)：可运行的 CLI 与 SDK 示例。

## 许可证

Copyright © LDC Labs

Licensed under the Apache License, Version 2.0.