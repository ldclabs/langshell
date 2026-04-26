# LangShell

> Stateful, capability-scoped, sandboxed code execution for AI agents.

**[English](./README.md) | [中文](./README_CN.md)**

LangShell 是一个面向 AI Agent 的安全执行层，目标是让 Agent 直接输出一段可验证、可恢复、可审计的 Python 代码来完成复杂任务，而不是把工作拆成大量脆弱的 tool calls。

项目以 Rust 实现，MVP 采用 Pydantic Monty 作为 Python 子集执行引擎。LangShell 的核心思想是：把代码作为接口，把 session 作为状态单元，把宿主显式注册的 capability 作为唯一外部世界入口。

## 当前状态

这个仓库目前仍处于启动骨架阶段。

- 已有 Cargo workspace、crate 拆分、Monty 依赖补丁与设计契约文档。
- 尚未完成运行时、CLI 二进制、内置工具、示例与端到端测试实现。
- [AGENTS.md](AGENTS.md) 是当前最完整的产品需求与工程契约来源。
- [SKILL.md](SKILL.md) 描述了 AI Agent 在未来如何安全地使用 LangShell。

README 以“产品说明 + 仓库导航 + 开发落地入口”为目标编写，并会明确区分“规划能力”和“当前已实现内容”。

## 为什么是 LangShell

传统 Agent 执行路径通常落在两种极端之间：

- tool-calling 过于碎片化，复杂逻辑需要多轮往返，成本高且恢复困难。
- 普通 shell 权限过大、状态脆弱、输出难解析，不适合运行不可信的 LLM 代码。

LangShell 试图提供中间层：

```text
AI tokens -> Python code -> safe execution -> structured result -> resumable state
```

它希望同时满足三类角色：

- AI Agent：能用熟悉的 Python 表达循环、条件、缓存、重试、并发和数据变换。
- Agent 框架开发者：能以稳定协议嵌入执行能力、注册工具、设置限制并收集审计信息。
- 平台 / 安全负责人：能保持默认零权限，让所有副作用都经过显式能力边界。

## 设计原则

- Code is the interface：对 Agent 而言，主要接口就是代码，而不是不断扩张的 tool schema。
- Session is the unit：状态、限制、审计、快照和生命周期都以 session 为中心。
- Capabilities over permissions：默认没有权限，所有外部能力都需要宿主显式注册。
- Every side effect is mediated：文件、网络、数据库等副作用都必须经过宿主能力。
- Errors are for agents：错误必须稳定、结构化、可用于自动修复与重试。

## 目标能力

LangShell 的 MVP 目标包括：

- 支持状态持久的 Python 子集执行。
- 支持 top-level await 与 async capability 调用。
- 支持 validate / dry-run，在不产生副作用的前提下发现语法、类型、权限和工具存在性问题。
- 支持 capability registry，让宿主以函数形式暴露受控能力。
- 支持 list_tools、describe_tool、current_policy 等能力发现接口。
- 支持结构化结果、stdout / stderr 捕获、诊断信息与错误码。
- 支持超时、取消、输出大小、内存与外部调用次数限制。
- 支持 snapshot / restore，用于中断恢复与审批边界暂停。

MVP 优先内置的能力是受控文件访问与受控 HTTP 访问，例如 read_text、write_text、list_dir、fetch_text、fetch_json。

## 架构概览

```text
Agent / Host App
		|
		+-- CLI
		+-- JSON-RPC Daemon
		+-- Rust SDK
						|
						v
			langshell-core
						|
		 +------+-------+
		 |              |
		 v              v
langshell-monty  langshell-tools
		 |
		 v
	 Monty VM
```

职责划分遵循以下边界：

- `langshell-core`：核心抽象，包括 session、policy、registry、snapshot、diagnostics 的稳定契约。
- `langshell-monty`：MVP 执行后端，封装所有 Monty 相关适配。
- `langshell-tools`：内置能力模块，例如文件与 HTTP 工具。
- `langshell-cli`：面向开发者的命令行入口，未来承载 run、validate、repl、daemon、session、tools 等命令。
- `langshell`：公共 Rust SDK，供宿主集成运行时、注册能力与发起执行。

## 仓库结构

```text
langshell/
├── monty/                  # 上游执行引擎 submodule
├── deno/                   # 未来 TypeScript / Deno backend submodule
├── crates/
│   ├── langshell/
│   ├── langshell-cli/
│   ├── langshell-core/
│   ├── langshell-monty/
│   └── langshell-tools/
├── docs/
├── AGENTS.md
├── SKILL.md
└── README.md
```

目前各 crate 仍是初始化骨架，但目录边界已经与产品文档中的工程契约保持一致，适合作为后续实现的落点。

## 目标接口示例

以下示例描述的是 LangShell 的目标使用体验，不代表当前仓库已经具备这些命令或行为。

### Agent 侧 Python

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

### CLI 目标形态

```bash
langshell run -e 'result = sum(range(10))' --json
langshell validate -f script.py --session-id agent-123
langshell daemon --listen unix:///tmp/langshell.sock
langshell session snapshot agent-123 --out snapshot.bin
```

### JSON-RPC 目标形态

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

## 稳定契约重点

根据当前设计文档，MVP 有几项必须尽早锁定的约束：

- 结果捕获优先级：优先读取全局变量 `result`，其次才是最后表达式值，再次才是 stdout。
- 错误码需要稳定且可机器解析，例如 `UNKNOWN_TOOL`、`PERMISSION_DENIED`、`RESULT_NOT_SERIALIZABLE`、`TIMEOUT_WALL`。
- snapshot 需要版本化，并校验 capability 集合，避免静默恢复到不兼容状态。
- 沙箱默认零权限，不提供宿主文件系统、环境变量、子进程或任意网络访问。

这些约束会直接影响 CLI、daemon、SDK 和测试矩阵的实现方式。

## 开发起步

### 环境要求

- Rust stable toolchain，且需要支持 Edition 2024。
- Git submodule。
- macOS、Linux、Windows 中任一受支持平台。

### 拉取仓库

```bash
git clone --recurse-submodules <repo-url>
cd langshell
```

如果已经克隆过仓库：

```bash
git submodule update --init --recursive
```

### 当前可做的基础检查

在仓库仍处于骨架阶段时，建议先运行：

```bash
cargo check
cargo test
```

随着运行时与 CLI 落地，再逐步补充 examples、e2e 与安全测试矩阵。

## 实现优先级建议

如果你准备从这个骨架开始推进 MVP，建议按以下顺序落地：

1. 在 `langshell-core` 中定义稳定的数据结构、错误码与 trait 边界。
2. 在 `langshell-monty` 中完成 Monty 执行适配与结果捕获。
3. 在 `langshell-tools` 中实现最小内置能力：文件读写、目录列举、HTTP 获取。
4. 在 `langshell` 中提供 Builder 与 session 运行接口。
5. 在 `langshell-cli` 中补齐 run、validate、daemon、session、tools 等命令。
6. 增加 snapshot、JSON-RPC、e2e 示例与安全测试矩阵。

## 文档入口

- [AGENTS.md](AGENTS.md)：产品需求、工程契约、错误码、snapshot 与测试矩阵。
- [SKILL.md](SKILL.md)：Agent 侧使用 LangShell 的方式、限制与最佳实践。

后续如果新增 RFC、API 参考或示例，建议统一放入 `docs/` 与 `examples/` 目录，并在 README 中持续链接。

## 路线图

### MVP

- Monty 集成。
- session 状态持久。
- 结构化结果与诊断输出。
- validate 模式。
- capability registry。
- 内置文件与 HTTP 能力。
- CLI、daemon IPC、Rust SDK 的最小可用链路。

### V1+

- durable snapshot store。
- 更完整的 typed stubs 与工具描述注入。
- SQLite / object_store 插件。
- TypeScript / Deno backend。
- 多租户 daemon 与远程执行能力。

## 许可证

Copyright © LDC Labs

Licensed under the Apache License, Version 2.0.