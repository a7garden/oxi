# oxicode WASM Extension Development Guide

This document describes how to develop, test, and distribute WASM extensions for oxicode.

## Overview

oxicode extensions are WebAssembly modules that run inside a sandboxed environment powered by [Extism](https://extism.org/). Extensions can:

- **Register tools** that the AI agent can call during conversations
- **Register commands** that users can invoke with `/command` in the TUI
- **Call host functions** to interact with the system (HTTP, files, commands, etc.)

Extensions are loaded from:

- `~/.oxicode/extensions/*.wasm` (global)
- `.oxicode/extensions/*.wasm` (project-local)

## Quick Start

### 1. Create a new Rust project

```bash
cargo init --lib my-extension
cd my-extension
```

### 2. Configure for WASM

```toml
# Cargo.toml
[package]
name = "my-extension"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
extism-pdk = "1.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### 3. Build target

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The output is at `target/wasm32-unknown-unknown/release/my_extension.wasm`.

### 4. Install

```bash
cp target/wasm32-unknown-unknown/release/my_extension.wasm ~/.oxicode/extensions/
```

Or use the CLI:

```bash
oxicode ext install owner/repo
```

---

## Extension Protocol

Every extension is a WASM module that exports some (or all) of the following functions. All input and output is JSON. Extism handles the marshalling.

### `init()` — Extension metadata

Called once when the extension is loaded. Returns metadata about the extension.

**Input:** `{}` (empty object)

**Output:**

```json
{
  "name": "my-extension",
  "version": "1.0.0",
  "description": "A short description of what this extension does",
  "permissions": ["fs_read", "network"]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique identifier (lowercase, hyphens) |
| `version` | string | Yes | Semantic version |
| `description` | string | No | Human-readable description |
| `permissions` | string[] | No | Requested permissions: `fs_read`, `fs_write`, `exec`, `env`, `network` |

If `init()` is not exported, the extension name is derived from the filename.

### `register_tools()` — Declare AI-callable tools

Called after `init()`. Returns the list of tools this extension provides.

**Input:** `{}`

**Output:**

```json
{
  "tools": [
    {
      "name": "web_search",
      "description": "Search the web for information",
      "schema": {
        "type": "object",
        "properties": {
          "query": {
            "type": "string",
            "description": "The search query"
          },
          "limit": {
            "type": "integer",
            "description": "Max results to return",
            "default": 5
          }
        },
        "required": ["query"]
      }
    }
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Tool name (must be unique across all extensions) |
| `description` | string | Yes | Description shown to the AI model |
| `schema` | object | Yes | JSON Schema for the tool parameters |

The `schema` follows [JSON Schema](https://json-schema.org/) draft-07. The AI model uses this to generate valid arguments.

### `execute_tool()` — Handle tool invocation

Called when the AI agent invokes one of your registered tools.

**Input:**

```json
{
  "tool": "web_search",
  "params": {
    "query": "rust wasm tutorial",
    "limit": 5
  }
}
```

**Output (success):**

```json
{
  "success": true,
  "output": "1. Rust and WebAssembly — https://rustwasm.github.io/book/\n2. ..."
}
```

**Output (error):**

```json
{
  "success": false,
  "output": "Search API rate limit exceeded. Try again later."
}
```

**Output (with metadata):**

```json
{
  "success": true,
  "output": "Found 3 results",
  "metadata": {
    "result_count": 3,
    "query_time_ms": 142
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `success` | boolean | No | Defaults to `true` if omitted |
| `output` | string | Yes | Text content returned to the AI |
| `metadata` | object | No | Arbitrary metadata attached to the result |

### `register_commands()` — Declare user commands

Called after `register_tools()`. Returns slash commands users can type in the TUI.

**Input:** `{}`

**Output:**

```json
{
  "commands": [
    {
      "name": "deploy",
      "description": "Deploy the current project to production"
    }
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Command name (user types `/deploy`) |
| `description` | string | Yes | Shown in `/help` output |

### `execute_command()` — Handle command invocation

Called when the user types `/command` in the TUI.

**Input:**

```json
{
  "command": "deploy",
  "args": "production --force"
}
```

**Output:**

```json
{
  "output": "Deployed to production at https://myapp.example.com"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `output` | string | Yes | Text displayed to the user |

If the output is not valid JSON with an `output` field, the raw string is displayed as-is.

---

## Host Functions

Extensions can call the following host functions to interact with the system. All functions use JSON-in/JSON-out via Extism.

### `oxicode_http_request` — Make HTTP requests

```json
// Input
{
  "url": "https://api.example.com/data",
  "method": "GET",
  "headers": {
    "Authorization": "Bearer token123"
  },
  "body": "optional request body"
}

// Output
{
  "status": 200,
  "headers": { "content-type": "application/json" },
  "body": "response body text"
}
```

**Methods supported:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`

**Limits:**
- Response body truncated at 1 MB
- SSRF protection blocks private IPs, localhost, and cloud metadata endpoints

### `oxicode_read_file` — Read a file

```json
// Input
{
  "path": "/path/to/file.txt",
  "offset": 0,
  "limit": 2000
}

// Output
{
  "success": true,
  "content": "file contents here...",
  "truncated": false,
  "bytes": 1234
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `path` | string | required | Absolute or relative path |
| `offset` | integer | 0 | Line number to start from |
| `limit` | integer | 2000 | Max lines to read |

**Limits:**
- Max 50 KB per read
- Blocked paths: `/etc`, `/sys`, `/proc`, `/dev`, `/boot`, `/root`, `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.kube`

### `oxicode_write_file` — Write a file

```json
// Input
{
  "path": "/path/to/output.txt",
  "content": "Hello, world!",
  "create_dirs": true
}

// Output
{
  "success": true,
  "bytes_written": 13
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `path` | string | required | File path to write |
| `content` | string | required | Content to write |
| `create_dirs` | boolean | true | Create parent directories if needed |

Same path restrictions as `oxicode_read_file` apply.

### `oxicode_exec` — Execute a command

```json
// Input
{
  "command": "npm",
  "args": ["test", "--verbose"],
  "cwd": "/path/to/project",
  "timeout": 30
}

// Output
{
  "success": true,
  "exit_code": 0,
  "stdout": "all tests passed\n",
  "stderr": "",
  "stdout_truncated": false,
  "stderr_truncated": false,
  "timed_out": false
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `command` | string | required | Binary to execute |
| `args` | string[] | [] | Arguments |
| `cwd` | string | "." | Working directory |
| `timeout` | integer | 30 | Timeout in seconds (max 120) |

**Blocked commands:** `sudo`, `su`, `doas`, `rm -rf /`, `mkfs`, `dd if=`, `chmod 777 /`, and other destructive patterns.

**Output limits:** stdout and stderr are each truncated at 10 KB.

### `oxicode_get_env` — Read environment variable

```json
// Input
{
  "key": "HOME"
}

// Output
{
  "success": true,
  "value": "/home/user"
}
```

**Blocked keys:** Anything containing `AWS_SECRET`, `PRIVATE_KEY`, `PASSWORD`, `TOKEN`, `SECRET` (case-insensitive).

### `oxicode_log` — Write to oxicode log

```
// Input: plain string (not JSON)
"Extension started successfully"

// No output
```

The input is a plain string, not a JSON object. It appears in oxicode's debug log output (not shown in TUI). Use `RUST_LOG=debug` to see it.

### `oxicode_kv_get` / `oxicode_kv_set` — Persistent key-value store

```json
// oxicode_kv_get input
{
  "key": "my_extension_state"
}

// oxicode_kv_get output
{
  "success": true,
  "value": "saved_data"
}

// oxicode_kv_set input
{
  "key": "my_extension_state",
  "value": "updated_data"
}
```

The KV store is in-memory and scoped to the current session. It is not persisted across restarts.

---

## Complete Example (Rust)

This example registers a single `echo` tool that returns its input.

```rust
// src/lib.rs
use extism_pdk::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
struct ToolDef {
    name: String,
    description: String,
    schema: serde_json::Value,
}

// ── Exported Functions ─────────────────────────────────────

#[plugin_fn]
pub fn init() -> FnResult<String> {
    Ok(json!({
        "name": "echo-tool",
        "version": "1.0.0",
        "description": "Echoes back the input text",
        "permissions": []
    }).to_string())
}

#[plugin_fn]
pub fn register_tools() -> FnResult<String> {
    Ok(json!({
        "tools": [{
            "name": "echo",
            "description": "Echoes back whatever text you provide",
            "schema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to echo back"
                    }
                },
                "required": ["text"]
            }
        }]
    }).to_string())
}

#[plugin_fn]
pub fn register_commands() -> FnResult<String> {
    Ok(json!({
        "commands": []
    }).to_string())
}

#[plugin_fn]
pub fn execute_tool(input: String) -> FnResult<String> {
    let req: serde_json::Value = serde_json::from_str(&input)
        .map_err(|e| anyhow::anyhow!("Invalid input: {}", e))?;

    let tool = req["tool"].as_str().unwrap_or("");
    let params = &req["params"];

    match tool {
        "echo" => {
            let text = params["text"].as_str().unwrap_or("(no text)");
            Ok(json!({
                "success": true,
                "output": text
            }).to_string())
        }
        _ => Ok(json!({
            "success": false,
            "output": format!("Unknown tool: {}", tool)
        }).to_string())
    }
}

#[plugin_fn]
pub fn execute_command(input: String) -> FnResult<String> {
    let _req: serde_json::Value = serde_json::from_str(&input)?;
    Ok(json!({
        "output": "No commands registered"
    }).to_string())
}
```

### Building and testing

```bash
# Build for WASM
cargo build --release --target wasm32-unknown-unknown

# Copy to extensions directory
cp target/wasm32-unknown-unknown/release/echo_tool.wasm ~/.oxicode/extensions/

# Start oxicode — the extension loads automatically
oxicode
```

When the AI agent calls the `echo` tool with `{"text": "hello"}`, the extension returns `"hello"`.

---

## Advanced Example: HTTP-based Tool

This extension calls an external API using `oxicode_http_request`:

```rust
#[plugin_fn]
pub fn execute_tool(input: String) -> FnResult<String> {
    let req: serde_json::Value = serde_json::from_str(&input)?;
    let tool = req["tool"].as_str().unwrap_or("");
    let params = &req["params"];

    match tool {
        "weather" => {
            let city = params["city"].as_str().unwrap_or("London");

            // Call host function to make HTTP request
            let http_req = json!({
                "url": format!("https://wttr.in/{}?format=3", city),
                "method": "GET"
            });

            // extism-pdk host_call: invoke an oxicode host function
            let response = extism_pdk::host_call(
                "oxicode_http_request",
                http_req.to_string().as_bytes(),
            ).map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

            let resp: serde_json::Value =
                serde_json::from_str(&String::from_utf8_lossy(&response))?;
            let body = resp["body"].as_str().unwrap_or("No data");

            Ok(json!({
                "success": true,
                "output": format!("Weather for {}: {}", city, body.trim())
            }).to_string())
        }
        _ => Ok(json!({
            "success": false,
            "output": format!("Unknown tool: {}", tool)
        }).to_string())
    }
}
```

---

## Distribution

### GitHub Releases

Package your `.wasm` file as a GitHub release asset:

```
my-extension/
  releases/
    v1.0.0/
      my-extension.wasm   ← attach this to the release
```

Users install with:

```bash
oxicode ext install owner/my-extension
oxicode ext install owner/my-extension@1.0.0   # specific version
```

### Registry

The local registry is stored at `~/.oxicode/extensions/registry.json`:

```json
{
  "extensions": {
    "my-extension": {
      "name": "my-extension",
      "version": "1.0.0",
      "source": "owner/my-extension",
      "installed_at": "2025-05-06T12:00:00Z",
      "wasm_path": "~/.oxicode/extensions/my-extension.wasm"
    }
  }
}
```

### CLI commands

```bash
oxicode ext install owner/repo       # Install latest release
oxicode ext install owner/repo@1.2.0 # Install specific version
oxicode ext list                      # List installed extensions
oxicode ext update                    # Update all extensions
oxicode ext update my-extension       # Update specific extension
oxicode ext remove my-extension       # Remove an extension
oxicode ext info owner/repo           # Show info without installing
```

---

## Security Model

### Sandboxing

- Extensions run inside a WASM sandbox with **no direct system access**
- Memory limited to 64 pages (4 MB)
- All system interaction goes through explicit host functions

### Host function restrictions

| Function | Restriction |
|----------|-------------|
| `oxicode_http_request` | Blocks private IPs, localhost, cloud metadata (169.254.169.254) |
| `oxicode_read_file` | Blocks system paths (`/etc`, `/sys`, `/proc`, `/dev`, `/boot`, `/root`) |
| `oxicode_read_file` | Blocks sensitive home dirs (`~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.kube`) |
| `oxicode_write_file` | Same path restrictions as read |
| `oxicode_exec` | Blocks `sudo`, `su`, `doas`, `rm -rf /`, `mkfs`, `dd if=`, etc. |
| `oxicode_exec` | Timeout capped at 120 seconds |
| `oxicode_get_env` | Blocks keys containing `SECRET`, `PASSWORD`, `TOKEN`, `PRIVATE_KEY` |
| `oxicode_http_request` | Response body truncated at 1 MB |
| `oxicode_read_file` | Max 50 KB per read |

### Permissions

The `permissions` field in `init()` is informational — it declares what the extension intends to use. Currently all host functions are available to all extensions regardless of declared permissions. Future versions may enforce permission gating.

---

## Language Support

Any language that compiles to WASM and works with Extism can be used:

| Language | SDK | Notes |
|----------|-----|-------|
| **Rust** | `extism-pdk` | Best supported, recommended |
| **Go** | `extism-pdk` + TinyGo | Requires TinyGo for small WASM output |
| **AssemblyScript** | `extism-as` | Native WASM, good for lightweight extensions |
| **C/C++** | Extism C SDK | Use Emscripten |
| **Zig** | Extism C SDK | Via C ABI |
| **Python** | `componentize-py` | Experimental, larger WASM output |

### Rust (recommended)

```toml
[dependencies]
extism-pdk = "1.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

### Go

```go
package main

import (
    "encoding/json"
    "github.com/extism/go-pdk"
)

// NOTE: In Go, "init" is a reserved function name.
// Use a different Go function name with //export to set the WASM export name.

//export init
func initExt() int32 {
    pdk.SetOutput([]byte(`{"name":"go-ext","version":"1.0.0"}`))
    return 0
}

//export register_tools
func registerTools() int32 {
    pdk.SetOutput([]byte(`{"tools":[{"name":"hello","description":"Says hello","schema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}]}`))
    return 0
}

//export execute_tool
func executeTool() int32 {
    input := string(pdk.Input())
    var req map[string]interface{}
    json.Unmarshal([]byte(input), &req)
    params := req["params"].(map[string]interface{})
    name := params["name"].(string)
    output, _ := json.Marshal(map[string]interface{}{
        "success": true,
        "output":  "Hello, " + name + "!",
    })
    pdk.SetOutput(output)
    return 0
}

func main() {}
```

Build:
```bash
tinygo build -o extension.wasm -target=wasi -no-debug -scheduler=none .
```

---

## Troubleshooting

### Extension not loading

1. Check the file is a valid `.wasm` file: `file extension.wasm`
2. Check it's in the right directory: `~/.oxicode/extensions/` or `.oxicode/extensions/`
3. Run oxicode with debug logging: `RUST_LOG=debug oxicode`
4. Look for `WASM extension loaded:` or `WASM extension error:` messages

### `init()` returns invalid JSON

The `init()` output must be valid JSON with at minimum `name` and `version` fields.

### Tool not appearing

1. `register_tools()` must return `{"tools": [...]}`
2. Each tool must have `name`, `description`, and `schema`
3. The `schema` must be a valid JSON Schema object

### Host function call fails

1. Check the input JSON matches the expected schema
2. Check for security restrictions (path blocked, env key blocked, etc.)
3. Use `oxicode_log` to debug from within the extension

### WASM memory errors

The memory limit is 64 pages (4 MB). If your extension needs more:
- Process data in smaller chunks
- Use streaming where possible
- Avoid loading large files entirely into memory
