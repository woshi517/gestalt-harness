# Gestalt Tools Crate (`gestalt-tools`)

Built-in tools and `ToolRegistry` for gestalt-harness. Provides the seven built-in agent tools, path validation, tool descriptor factories, and response shaping.

---

## Built-in Tools

| Tool | Risk | Read-Only | Idempotent | Retry | Parallel |
|------|------|-----------|------------|-------|----------|
| `read` | Low | yes | yes | 2 retries, 100ms | yes |
| `search` | Low | yes | yes | 2 retries, 100ms | yes |
| `find_files` | Low | yes | yes | 2 retries, 100ms | yes |
| `write` | Medium | no | no | none | no |
| `patch` | Medium | no | no | none | no |
| `bash` | Classified | no | no | none | Low only |
| `web_fetch` | Medium | yes | yes | 2 retries, 200ms | no |

### `read` — Read File

Reads a file with optional line range and token limit. Rejects paths outside the workspace, symlink escapes, and secret-pattern filenames (`.env`, `.key`, `.pem`, etc.).

```rust
pub struct ReadInput {
    pub path: String,
    pub start_line: usize,
    pub end_line: Option<usize>,
    pub max_tokens: usize,
}
```

### `search` — Search Files

Searches workspace text files with local, path-scoped semantics using regular expressions or literal strings. Supports glob filtering, case-insensitive mode, context lines before/after, hidden-file controls, gitignore controls, and result limits.

```rust
pub struct SearchInput {
    pub pattern: String,
    pub path: Option<String>,
    pub file_glob: Option<String>,
    pub case_insensitive: bool,
    pub is_regex: bool,
    pub context_before: usize,
    pub context_after: usize,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    pub max_results: usize,
}
```

### `find_files` — Fuzzy Find Files

Fuzzy finds files inside the workspace relative to a directory root. Excludes hidden files and respects `.gitignore` rules by default.

```rust
pub struct FindFilesInput {
    pub query: String,
    pub path: Option<String>,
    pub file_glob: Option<String>,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    pub max_results: usize,
}
```

### `write` — Write File

Writes full replacement content to a file with optional change preview, parent directory creation, conflict checks (`expected_hash`), and dry runs (`dry_run`). Fails fast when the parent directory is missing and `create_dirs` is false. If the existing file is not valid UTF-8, it rejects the write explicitly. If the file's content is unchanged, it skips writing to disk to optimize execution.

```rust
pub struct WriteInput {
    pub path: String,
    pub content: String,
    pub show_diff: bool,
    pub create_dirs: bool,
    pub expected_hash: Option<String>,
    pub dry_run: bool,
}
```

**Output JSON Structure:**
Returns a structured JSON summary instead of passing through default response shaping:
```json
{
  "path": "path/to/file",
  "bytes_written": 120,
  "status": "updated",
  "diff": "...",
  "diff_truncated": false,
  "lines_added": 5,
  "lines_removed": 2,
  "dry_run": false
}
```
* **Change Preview:** `diff` contains a bounded human-readable line-based minimal unified diff. This preview may be truncated with an explicit placeholder if it exceeds 200 lines or 16 KB.
* **Metadata:** Contains structured counts for added/removed lines, written byte count, write status (`created`, `updated`, or `unchanged`), and truncation flags.

### `patch` — Apply Patch

Applies a high-level patch document to workspace files, expressing operations like Add, Update, Delete, or Move directly. Supports optional conflict checks (`expected_hash`) and dry runs (`dry_run`).

```rust
pub struct PatchInput {
    pub path: String,
    pub patch: String,
    pub expected_hash: Option<String>,
    pub dry_run: bool,
}
```

The patch document uses block envelopes to represent operations:

```text
<<< ADD FILE: path/to/file >>>
file contents here
<<< END ADD FILE >>>

<<< UPDATE FILE: path/to/file >>>
<<<<<<< SEARCH
old content
=======
new content
>>>>>>>
<<< END UPDATE FILE >>>

<<< DELETE FILE: path/to/file >>>
<<< END DELETE FILE >>>

<<< MOVE FILE: path/to/old >>>
<<< TO: path/to/new >>>
<<< END MOVE FILE >>>
```

**Key Safety and Execution Characteristics:**
- **Expected Hash Check:** The `expected_hash` check is evaluated once against the primary target file (`path`) before any operations are validated or applied, preventing destructive actions (e.g. Delete, Move) or content updates on stale files.
- **Failure Atomicity:** Staged failure-atomically. Writes are staged to temporary files alongside destination paths. If any write or validation fails, all temp files are cleaned up, leaving the workspace completely unmodified.
- **Ordered Sequence Capabilities:** Allows delete-then-recreate flows (e.g., delete `b.txt`, then move `a.txt` to `b.txt`) inside the same patch document.
- **Post-Mutation Verification:** Structural parser is shared with the verification engine. Deleted files and moved-from source paths are filtered out, so post-mutation verification (e.g. `FileExistsVerifier`) only inspects files that should exist.

### `bash` — Execute Shell Command

Executes a shell command with a configurable timeout and working directory. Classifies risk per command:

- **Critical:** `rm -rf`, `mkfs`, `dd if=`, fork bombs, `chmod 777`, secret paths
- **High:** SSH, Docker, `git push`, `curl`, `wget`, `sudo`, `python -c`, shell metacharacters, `/dev/tcp`
- **Medium:** `rm`, `mv`, `cp`, `mkdir`, installers (`apt`, `brew`, `pip`)
- **Low:** `ls`, `cat`, `grep`, `rg`, `find`, `cargo check`, `git status`, `git diff`

```rust
pub struct BashInput {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}
```

### `web_fetch` — Fetch Web Content

Fetches and returns text content from a URL. DNS-level private IP blocking, only http/https schemes allowed, safe redirect following, token-limited text extraction from HTML.

```rust
pub struct WebFetchInput {
    pub url: String,
    pub max_tokens: usize,
    pub raw: bool,
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

Rejects duplicate tool names. `default_registry()` provides the seven built-in tools pre-registered.

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
- **write / patch:** Natively return structured JSON summaries containing path, diff (for write), and execution metadata rather than passing through response shaping.

Shaped output keeps session history compact while still communicating enough detail for the model to reason about results.

---

## Quick Start

```rust
use std::sync::Arc;
use gestalt_tools::default_registry;

let tools = default_registry(); // Arc<dyn ToolCatalog> with 7 tools
```
