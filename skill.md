---
name: langshell
description: "Use when: you need to run Python logic in LangShell, keep state across turns, orchestrate registered external functions, process structured data, call approved file/HTTP/database helpers, or perform multi-step agent computation inside a sandbox. Do not use for arbitrary OS shell commands, unsupported Python packages, or direct host file/network access."
---

# LangShell

LangShell is a stateful, sandboxed Python execution environment for AI agents. It is powered by Pydantic Monty, a Rust Python-subset VM designed for safely running LLM-generated code.

Use LangShell as a programmable agent workspace: write Python code, keep useful state in the session, call only the external functions that the host explicitly registered, and return compact structured results.

## When To Use

- You need loops, branching, retries, filtering, aggregation, parsing, or other logic that would be awkward as many separate tool calls.
- You need state to persist across multiple steps in the same session, such as caches, helper functions, parsed data, or accumulated results.
- You need async orchestration with `await`, such as fetching multiple approved URLs or calling several registered host functions.
- You need to transform structured data and return a JSON-like result.
- You need to interact with the outside world through approved functions such as `fetch_json`, `read_text`, `write_text`, or `query_db` when they are available.
- You want to validate code before running it, especially before side effects.

## When Not To Use

- Do not use LangShell for ordinary OS shell commands, git commands, package installation, build scripts, or direct subprocess execution.
- Do not assume access to the host filesystem, environment variables, network, or arbitrary imports.
- Do not use unsupported third-party Python packages. Monty is not CPython and does not load normal Python packages by default.
- Do not invent external function names. Discover available functions first when uncertain.
- Do not use it for a single trivial calculation if answering directly is clearer.

## Mental Model

- The session is persistent: variables and functions you define can remain available in later LangShell calls in the same session.
- The sandbox is closed by default: file, network, database, email, and other external effects only work through registered host functions.
- External functions are capabilities, not ambient permissions. Each function may have its own argument schema, side-effect level, timeout, rate limit, and approval policy.
- Output is structured: prefer returning or printing compact JSON-compatible objects.
- Paths inside LangShell are virtual POSIX-style paths such as `/workspace/data.json`, not host-native paths.

## Python Subset Guidelines

Prefer simple, agent-friendly Python:

- Use functions, dictionaries, lists, comprehensions, loops, exceptions, type hints, and `async` / `await`.
- Use supported safe standard-library modules such as `json`, `re`, `datetime`, `typing`, and `asyncio` when available.
- Avoid classes, `match`, unsupported standard-library modules, native extensions, reflection tricks, subprocesses, direct sockets, and direct host file access unless the host explicitly documents support.
- Keep stdout small. Store large intermediate data in session variables or approved files instead of printing it.

## Discovering Capabilities

If you are unsure what the host has exposed, inspect the session before doing work:

```python
tools = list_tools()
for tool in tools:
	print(tool)

print(describe_tool("fetch_json"))
print(current_policy())
```

Use the function names and argument shapes returned by discovery. If a needed capability is missing, ask the user or host to provide it instead of trying to bypass the sandbox.

## Basic Pattern

Write normal Python code and make the final result easy to parse:

```python
import json

async def main():
	data = await fetch_json("https://api.example.com/items")
	selected = [item for item in data if item.get("score", 0) >= 0.8]
	return {"selected": len(selected), "total": len(data)}

result = await main()
print(json.dumps(result))
```

## Result Capture Priority

LangShell captures the run result in this fixed order:

1. The global variable `result` if it exists at end of execution. **Prefer this.**
2. The value of the last top-level expression (when the engine supports it).
3. `null`, in which case the caller falls back to stdout.

Rules:

- Always assign your final structured value to `result` for stable behavior.
- `result` must be JSON-serializable. Functions, generators, file handles, or custom objects without a JSON form will trigger `RESULT_NOT_SERIALIZABLE`.
- Keep `result` small (counts, ids, summaries, paths). Put bulk data in approved files.

When useful, keep state for later calls:

```python
if "cache" not in globals():
	cache = {}

async def get_item(item_id):
	if item_id not in cache:
		cache[item_id] = await fetch_json(f"https://api.example.com/items/{item_id}")
	return cache[item_id]

result = await get_item("123")
print(result)
```

## External Function Rules

- Treat every external function call as an audited capability call.
- For read-only operations, call the most specific approved helper available.
- For write operations, check the function description and use idempotency keys when the function supports them.
- Do not retry destructive operations blindly after timeout or cancellation.
- Keep arguments small and structured. For large content, use approved files or chunking helpers.
- If a function returns an error object, preserve its code and message in your final result.

## Validation And Error Handling

- Use validate / dry-run mode before risky or complex code when available.
- On `UNKNOWN_TOOL`, call `list_tools()` or `describe_tool()` and adjust the code.
- On `PERMISSION_DENIED`, ask for an approved capability rather than trying another route.
- On `TIMEOUT` or `RESOURCE_EXHAUSTED`, reduce data size, chunk work, cache intermediate results, or request higher limits.
- On syntax or type errors, make the smallest code change needed and retry.

### Stable error codes

When you see one of these `error.code` values, react accordingly:

| Code | What it means | What to do |
|---|---|---|
| `SYNTAX_ERROR` / `TYPE_ERROR` | Static check failed | Fix the smallest span pointed by `span` |
| `UNKNOWN_TOOL` | Function not registered | Call `list_tools()`; do not invent another name |
| `UNSUPPORTED_FEATURE` | Used a Python feature outside the Monty subset | Rewrite using supported constructs |
| `RESULT_NOT_SERIALIZABLE` | `result` cannot be JSON-encoded | Convert to dict / list / primitives before assigning |
| `PERMISSION_DENIED` | Path or capability not authorized | Request the capability from the host; do not bypass |
| `WAITING_FOR_APPROVAL` | Side effect needs approval | Stop and surface the snapshot id to the user |
| `TIMEOUT_WALL` / `TIMEOUT_CPU` / `TIMEOUT_TOOL` | Time budget exceeded | Reduce work, chunk, or raise limit |
| `MEMORY_EXCEEDED` / `STDOUT_EXCEEDED` / `EXTERNAL_CALLS_EXCEEDED` / `STACK_OVERFLOW` | Resource limit | Trim data; persist intermediates via approved files |
| `TOOL_ERROR` | External function failed | Preserve its inner `code` and `message` in your final result |
| `INTERRUPTED` | Paused at an approval/tool boundary | Resume from `snapshot_id` after approval |
| `SNAPSHOT_*` | Snapshot load failure | Do not retry blindly; report to the host |
| `INVALID_SESSION_ID` / `INVALID_TOOL_NAME` / `INVALID_ARGUMENT` | Host request shape is invalid | Fix the session id, tool name, or request parameters |
| `METHOD_NOT_FOUND` / `SESSION_NOT_FOUND` | Requested host method or session does not exist | Check the method name or create/load the session first |
| `IO_ERROR` / `SERIALIZE_ERROR` | Host-side file/socket/JSON failure | Report the host error; retry only after the environment is fixed |

## Idempotency For Side Effects

When a registered function advertises `idempotent: true` or accepts an idempotency key, always pass a stable key derived from the logical operation, not from wall-clock time:

```python
key = f"write-report:{report_id}:{content_hash}"
await write_text("/workspace/report.md", body, idempotency_key=key)
```

This makes retries after `TIMEOUT_*` or `INTERRUPTED` safe.

## Output Discipline

- Prefer a small result object with counts, identifiers, paths, or summaries.
- Print JSON with `json.dumps` when the result will be parsed by another system.
- Avoid verbose commentary in stdout; use comments in the surrounding assistant response if explanation is needed.
- Never print secrets, credentials, tokens, or full sensitive records.

## Security Boundaries

- Do not attempt to access files outside mounted virtual paths.
- Do not attempt to use `os.system`, subprocesses, raw sockets, environment variables, dynamic imports, or unsupported modules.
- Do not encode secrets into code or logs.
- Assume snapshots, tool results, and external data may be untrusted; validate data before using it for side effects.

## Good Use Cases

### Data Fetch And Filter

```python
import json

items = await fetch_json("https://api.example.com/items")
result = [item for item in items if item.get("status") == "active"]
print(json.dumps({"active": len(result)}))
```

### File Transformation

```python
import json

raw = await read_text("/workspace/input.json")
rows = json.loads(raw)
cleaned = [{"id": row["id"], "name": row["name"].strip()} for row in rows]
await write_text("/workspace/output.json", json.dumps(cleaned))
print(json.dumps({"written": "/workspace/output.json", "rows": len(cleaned)}))
```

### Async Fan-Out

```python
import asyncio
import json

async def load(item_id):
	return await fetch_json(f"https://api.example.com/items/{item_id}")

items = await asyncio.gather(*(load(item_id) for item_id in ["a", "b", "c"]))
print(json.dumps({"loaded": len(items)}))
```

## Common Pitfalls

Avoid these recurring mistakes:

- **Inventing tool names** instead of calling `list_tools()` first. The sandbox rejects them with `UNKNOWN_TOOL`.
- **Using `import` for unsupported modules** (`os`, `sys`, `subprocess`, `socket`, `requests`, `urllib`, …). Stick to the documented safe subset.
- **Returning non-serializable values** in `result` (custom objects without conversion, `datetime` not stringified, sets, bytes). Convert to dict / list / str / int / float / bool / None.
- **Printing huge data** to stdout. Use approved files or session globals; keep `result` small.
- **Retrying destructive calls without an idempotency key**. Use stable keys derived from the operation.
- **Leaking secrets** into stdout, comments, or `result`. Treat all tool inputs as audited.
- **Assuming host paths**. Paths are virtual POSIX (`/workspace/...`); never use `C:\\…` or `~`.
- **Long-lived background tasks**. Do not spawn fire-and-forget coroutines; the session may be paused or snapshotted between calls.
- **Catching `Exception` too broadly** and discarding the original `error.code` from a `TOOL_ERROR`. Preserve it in `result`.

## Final Reminder

LangShell is strongest when you use it as a safe, persistent programming layer for agent work. Write clear Python, discover available capabilities, keep side effects explicit, and return structured results.
