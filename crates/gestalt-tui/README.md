# Gestalt TUI Crate (`gestalt-tui`)

The `gestalt-tui` crate provides the Ratatui-based interactive terminal UI for the gestalt harness. It operates purely as a client shell on top of `gestalt-app` and `gestalt-runtime`.

---

## Ownership Boundary

- **What it owns:** UI widget layout, terminal event listening (keyboard/resize), scroll buffers, and presentation logs.
- **What it does NOT own:** Business logic, task execution loops, policy evaluation, or file/credential I/O.

---

## Core Entry Points

- `gestalt_tui::run_tui` — Launches the terminal screen and enters the main UI event loop.

---

## Construction Example

Launch the TUI programmatically:

```rust
use gestalt_tui::run_tui;

// Launch the interactive screen, taking control of stdout
if let Err(e) = run_tui() {
    eprintln!("TUI encountered an error: {:?}", e);
}
```

---

## Cancellation & Failure Semantics

- Pressing `Esc` or sending a cancellation signal gracefully releases raw terminal mode and exits.
- Panics are intercepted via a custom panic hook to restore standard terminal mode before printing backtraces.

---

## Feature Gates

- This crate is compiled conditionally behind the `tui` cargo feature flag.
