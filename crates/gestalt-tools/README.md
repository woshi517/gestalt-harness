# Gestalt Tools Crate (`gestalt-tools`)

Built-in tools and `ToolRegistry` for gestalt-harness. Provides the six built-in agent tools, path validation, tool descriptor factories, and response shaping.

---

## Built-in Tools

| Tool | Risk | Read-Only | Idempotent | Retry | Parallel |
|------|------|-----------|------------|-------|----------|
| `read` | Low | yes | yes | 2 retries, 100ms | yes |
| `search` | Low | yes | yes | 2 retries, 100ms | yes |
| `write` | Medium | no | no | none | no |
| `patch` | Medium | no | no | none | no |
| `bash` | Classified | no | no | none | Low only |
| `web_fetch` | Medium | yes | yes | 2 retries, 200ms | no |

### `read` — Read File

Reads a file with optional line range and token limit. Rejects paths outside the workspace, symlink escapes, and secret-pattern filenames (`.env`, `.key`, `.pem`, etc.).

```rust
pub struct ReadInput {
    pub path: PathBuf,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub max_tokens: Option<usize>,
}
```

### `search` — Search Files

Searches workspace files with regex patterns. Supports glob filtering, case-insensitive mode, and result limits.

```rust
pub struct SearchInput {
    pub pattern: String,
    pub path: Option<PathBuf>,
    pub file_glob: Option<String>,
    pub case_insensitive: Option<bool>,
    pub max_results: Option<usize>,
}
```

### `write` — Write File

Writes content to a file with optional diff display and directory creation. Fails fast when the parent directory is missing and `create_dirs` is false.

```rust
pub struct WriteInput {
    pub path: PathBuf,
    pub content: String,
    pub show_diff: Option<bool>,
    pub create_dirs: Option<bool>,
}
```

### `patch` — Apply Patch

Applies a unified diff to a file. Returns the full patched content on success; structured error with context mismatch detection on failure.

```rust
pub struct PatchInput {
    pub path: PathBuf,
    pub patch: String,
}
```

### `bash` — Execute Shell Command

Executes a shell command with a configurable timeout and working directory. Classifies risk per command:

- **Critical:** `rm -rf`, `mkfs`, `dd if=`, fork bombs, `chmod 777`, secret paths
- **High:** SSH, Docker, `git push`, `curl`, `wget`, `sudo`, `python -c`, shell metacharacters, `/dev/tcp`
- **Medium:** `rm`, `mv`, `cp`, `mkdir`, installers (`apt`, `brew`, `pip`)
- **Low:** `ls`, `cat`, `grep`, `rg`, `find`, `cargo check`, `git status`, `git diff`

```rust
pub struct BashInput {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub timeout_secs: Option<u64>,
}
```

### `web_fetch` — Fetch Web Content

Fetches and returns text content from a URL. DNS-level private IP blocking, only http/https schemes allowed, safe redirect following, token-limited text extraction from HTML.

```rust
pub struct WebFetchInput {
    pub url: String,
    pub max_tokens: Option<usize>,
    pub raw: Option<bool>,
}
```

---

## Path Validation (`path.rs`)

Every filesystem tool routes through path validation:

- **Workspace boundary:** `canonicalize()` prevents `../` traversal. Rejects paths outside the workspace root.
- **Secret rejection:** Refuses paths containing `secret`, `credential`, `.env`, `.key`, `.pem`, `token`, `password`.
- **Symlink escape:** Resolves symlinks and checks the target against workspace boundaries.
- **Ancestor existence:** For write operations with `create_dirs`, ensures parent directories exist or can be created within workspace bounds.

---

## Tool Registry (`registry.rs`)

`ToolRegistry` implements `ToolCatalog` and serves as the standard container for built-in and extension tools:

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()>;
}
```

Rejects duplicate tool names. `default_registry()` provides the six built-in tools pre-registered.

---

## Tool Descriptors (`builtin_descriptors.rs`)

`make_builtin_descriptor()` constructs a `ToolDescriptor` for each built-in tool with `BuiltInTrusted` annotations. Only built-in tools get automatic trust — extension tools must pass through the trust gate in `gestalt-runtime`.

Each built-in descriptor includes:
- `read_only` and `idempotent` annotations (sourced as `BuiltInTrusted`)
- An optional `ToolRetryPolicy` (only `read`, `search`, and `web_fetch` include one)
- `ProviderToolFormat::Text` response contract

---

## Response Shaping (`response_shaping.rs`)

`shape_tool_response()` formats tool output before it enters the session history:

- **read:** Includes path, line range, and truncation notice when output exceeds token limit.
- **search:** Includes result count, matched lines with line numbers, and file references.
- **bash:** Includes exit status, stderr/stdout summary, duration, and truncation state.
- **web_fetch:** Includes fetch duration, byte count, and truncation notice.
- **write / patch:** Includes diff display and byte count.

Shaped output keeps session history compact while still communicating enough detail for the model to reason about results.

---

## Quick Start

```rust
use std::sync::Arc;
use gestalt_tools::default_registry;

let tools = default_registry(); // Arc<dyn ToolCatalog> with 6 tools
```
