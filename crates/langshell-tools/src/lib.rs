use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use langshell_core::{
    Capability, RegisteredTool, SideEffect, ToolCallContext, ToolError, ToolFuture, ToolRegistry,
};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct FileMount {
    pub virtual_path: String,
    pub host_path: PathBuf,
    pub writable: bool,
}

impl FileMount {
    pub fn readonly(virtual_path: impl Into<String>, host_path: impl Into<PathBuf>) -> Self {
        Self {
            virtual_path: normalize_virtual_root(&virtual_path.into()),
            host_path: host_path.into(),
            writable: false,
        }
    }

    pub fn readwrite(virtual_path: impl Into<String>, host_path: impl Into<PathBuf>) -> Self {
        Self {
            virtual_path: normalize_virtual_root(&virtual_path.into()),
            host_path: host_path.into(),
            writable: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolConfig {
    pub file_mounts: Vec<FileMount>,
    pub http_allowlist: Vec<String>,
}

pub fn register_builtin_tools(
    registry: &mut ToolRegistry,
    config: ToolConfig,
) -> Result<(), langshell_core::ErrorObject> {
    if !config.file_mounts.is_empty() {
        register_file_tools(registry, config.file_mounts)?;
    }
    if !config.http_allowlist.is_empty() {
        register_http_tools(registry, config.http_allowlist)?;
    }
    register_discovery_tools(registry)?;
    Ok(())
}

pub fn register_discovery_tools(
    registry: &mut ToolRegistry,
) -> Result<(), langshell_core::ErrorObject> {
    let list_capability = Capability::new(
        "list_tools",
        "List capabilities registered in this session.",
        SideEffect::None,
    );
    let describe_capability = Capability::new(
        "describe_tool",
        "Describe one registered capability by name.",
        SideEffect::None,
    );
    let policy_capability = Capability::new(
        "current_policy",
        "Return the current sandbox policy summary.",
        SideEffect::None,
    );

    let mut list_capabilities = registry.capabilities();
    list_capabilities.push(list_capability.clone());
    list_capabilities.push(describe_capability.clone());
    list_capabilities.push(policy_capability.clone());
    let list_tool = RegisteredTool::sync(list_capability, move |_| Ok(json!(list_capabilities)));
    registry.register(list_tool)?;

    let describe_capabilities = Arc::new(list_capabilities_for_describe(
        registry,
        &describe_capability,
        &policy_capability,
    ));
    let describe_tool = RegisteredTool::sync(describe_capability, move |ctx| {
        let name = first_string_arg(&ctx, "describe_tool")?;
        describe_capabilities
            .iter()
            .find(|capability| capability.name == name)
            .map(|capability| json!(capability))
            .ok_or_else(|| {
                ToolError::new(
                    "UNKNOWN_TOOL",
                    format!("Function {name} is not registered."),
                )
            })
    });
    registry.register(describe_tool)?;

    let policy_capabilities = list_capabilities_for_policy(registry, &policy_capability);
    let current_policy = RegisteredTool::sync(policy_capability, move |_| {
        Ok(json!({
            "default_permissions": "none",
            "filesystem": "capability_only",
            "network": "capability_only",
            "subprocess": "denied",
            "tools": policy_capabilities,
        }))
    });
    registry.register(current_policy)?;
    Ok(())
}

fn list_capabilities_for_describe(
    registry: &ToolRegistry,
    describe_capability: &Capability,
    policy_capability: &Capability,
) -> Vec<Capability> {
    let mut capabilities = registry.capabilities();
    capabilities.push(describe_capability.clone());
    capabilities.push(policy_capability.clone());
    capabilities
}

fn list_capabilities_for_policy(
    registry: &ToolRegistry,
    policy_capability: &Capability,
) -> Vec<Capability> {
    let mut capabilities = registry.capabilities();
    capabilities.push(policy_capability.clone());
    capabilities
}

pub fn register_file_tools(
    registry: &mut ToolRegistry,
    mounts: Vec<FileMount>,
) -> Result<(), langshell_core::ErrorObject> {
    let mounts = Arc::new(mounts);

    let read_mounts = mounts.clone();
    registry.register(RegisteredTool::sync(
        Capability::new(
            "read_text",
            "Read UTF-8 text from an authorized virtual path.",
            SideEffect::Read,
        ),
        move |ctx| {
            let virtual_path = first_string_arg(&ctx, "read_text")?;
            let resolved = resolve_virtual_path(&read_mounts, &virtual_path, false)?;
            fs::read_to_string(&resolved)
                .map(Value::String)
                .map_err(|err| {
                    ToolError::new("TOOL_ERROR", format!("read_text({virtual_path}): {err}"))
                })
        },
    ))?;

    let write_mounts = mounts.clone();
    registry.register(RegisteredTool::sync(
        Capability::new(
            "write_text",
            "Write UTF-8 text to an authorized writable virtual path.",
            SideEffect::Write,
        ),
        move |ctx| {
            let virtual_path = first_string_arg(&ctx, "write_text")?;
            let text = ctx.args.get(1).and_then(Value::as_str).ok_or_else(|| {
                ToolError::new("TYPE_ERROR", "write_text requires path and text arguments.")
            })?;
            let resolved = resolve_virtual_path(&write_mounts, &virtual_path, true)?;
            if let Some(parent) = resolved.parent() {
                fs::create_dir_all(parent).map_err(|err| {
                    ToolError::new("TOOL_ERROR", format!("creating parent directory: {err}"))
                })?;
            }
            fs::write(&resolved, text).map_err(|err| {
                ToolError::new("TOOL_ERROR", format!("write_text({virtual_path}): {err}"))
            })?;
            Ok(json!({"path": virtual_path, "bytes": text.len()}))
        },
    ))?;

    let list_mounts = mounts;
    registry.register(RegisteredTool::sync(
        Capability::new(
            "list_dir",
            "List direct children of an authorized virtual directory.",
            SideEffect::Read,
        ),
        move |ctx| {
            let virtual_path = first_string_arg(&ctx, "list_dir")?;
            let resolved = resolve_virtual_path(&list_mounts, &virtual_path, false)?;
            let mut entries = Vec::new();
            for entry in fs::read_dir(&resolved).map_err(|err| {
                ToolError::new("TOOL_ERROR", format!("list_dir({virtual_path}): {err}"))
            })? {
                let entry = entry.map_err(|err| {
                    ToolError::new("TOOL_ERROR", format!("list_dir entry: {err}"))
                })?;
                entries.push(entry.file_name().to_string_lossy().to_string());
            }
            entries.sort();
            Ok(json!(entries))
        },
    ))?;

    Ok(())
}

pub fn register_http_tools(
    registry: &mut ToolRegistry,
    allowlist: Vec<String>,
) -> Result<(), langshell_core::ErrorObject> {
    let allowlist = Arc::new(allowlist);
    let text_allowlist = allowlist.clone();
    registry.register(RegisteredTool::asynchronous(
        Capability::new(
            "fetch_text",
            "Fetch text from an allowlisted HTTP(S) URL.",
            SideEffect::Network,
        ),
        move |ctx| {
            let allowlist = text_allowlist.clone();
            Box::pin(async move {
                let url = first_string_arg(&ctx, "fetch_text")?;
                ensure_url_allowed(&allowlist, &url)?;
                Err(ToolError::new(
                    "TOOL_ERROR",
                    "fetch_text transport is not configured in this MVP build.",
                ))
            }) as ToolFuture
        },
    ))?;

    let json_allowlist = allowlist;
    registry.register(RegisteredTool::asynchronous(
        Capability::new("fetch_json", "Fetch JSON from an allowlisted HTTP(S) URL.", SideEffect::Network),
        move |ctx| {
            let allowlist = json_allowlist.clone();
            Box::pin(async move {
                let url = first_string_arg(&ctx, "fetch_json")?;
                ensure_url_allowed(&allowlist, &url)?;
                Err(ToolError::new(
                    "TOOL_ERROR",
                    "fetch_json transport is not configured in this MVP build; register a host fetch_json capability.",
                ))
            }) as ToolFuture
        },
    ))?;

    Ok(())
}

fn first_string_arg(ctx: &ToolCallContext, function: &str) -> Result<String, ToolError> {
    ctx.args
        .first()
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ToolError::new(
                "TYPE_ERROR",
                format!("{function} requires a string first argument."),
            )
        })
}

fn normalize_virtual_root(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

fn resolve_virtual_path(
    mounts: &[FileMount],
    virtual_path: &str,
    write: bool,
) -> Result<PathBuf, ToolError> {
    if virtual_path.as_bytes().contains(&0) || !virtual_path.starts_with('/') {
        return Err(ToolError::new(
            "PERMISSION_DENIED",
            format!("Path {virtual_path:?} is not an absolute virtual path."),
        ));
    }

    let mount = mounts
        .iter()
        .filter(|mount| {
            virtual_path == mount.virtual_path
                || virtual_path.starts_with(&format!("{}/", mount.virtual_path))
        })
        .max_by_key(|mount| mount.virtual_path.len())
        .ok_or_else(|| {
            ToolError::new(
                "PERMISSION_DENIED",
                format!("No mount authorizes {virtual_path}."),
            )
        })?;

    if write && !mount.writable {
        return Err(ToolError::new(
            "PERMISSION_DENIED",
            format!("Mount {} is read-only.", mount.virtual_path),
        ));
    }

    let suffix = virtual_path
        .strip_prefix(&mount.virtual_path)
        .unwrap_or(virtual_path)
        .trim_start_matches('/');
    let suffix_path = Path::new(suffix);
    if suffix_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ToolError::new(
            "PERMISSION_DENIED",
            format!("Path traversal is not allowed: {virtual_path}."),
        ));
    }

    let host_root = mount.host_path.canonicalize().map_err(|err| {
        ToolError::new(
            "PERMISSION_DENIED",
            format!("Mount root is not accessible: {err}"),
        )
    })?;
    let candidate = host_root.join(suffix_path);

    if candidate.exists() {
        let canonical = candidate.canonicalize().map_err(|err| {
            ToolError::new(
                "PERMISSION_DENIED",
                format!("Path is not accessible: {err}"),
            )
        })?;
        if !canonical.starts_with(&host_root) {
            return Err(ToolError::new(
                "PERMISSION_DENIED",
                format!("Path escapes mount boundary: {virtual_path}."),
            ));
        }
        Ok(canonical)
    } else {
        let parent = candidate.parent().unwrap_or(&host_root);
        let canonical_parent = parent.canonicalize().map_err(|err| {
            ToolError::new(
                "PERMISSION_DENIED",
                format!("Parent path is not accessible: {err}"),
            )
        })?;
        if !canonical_parent.starts_with(&host_root) {
            return Err(ToolError::new(
                "PERMISSION_DENIED",
                format!("Path escapes mount boundary: {virtual_path}."),
            ));
        }
        Ok(candidate)
    }
}

fn ensure_url_allowed(allowlist: &[String], url: &str) -> Result<(), ToolError> {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return Err(ToolError::new(
            "PERMISSION_DENIED",
            "Only http:// and https:// URLs are allowed.",
        ));
    };
    let host = rest
        .split('/')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default();
    if allowlist.iter().any(|allowed| allowed == host) {
        Ok(())
    } else {
        Err(ToolError::new(
            "PERMISSION_DENIED",
            format!("Host {host} is not in the HTTP allowlist."),
        ))
    }
}
