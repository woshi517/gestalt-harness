# TUI Screen and Widget Design System

This document outlines the decoupled, Elm-style architecture used for the Gestalt TUI presentation layer, specifying focus routing, viewport policies, keyboard mapping priorities, and pure widgets rendering.

---

## 1. Elm-Style Component Architecture

The TUI architecture decouples state, transitions, views, and side effects. All user interactions flow through a unidirectional loop:

```
  ┌──────────────────────────────────────────────────────────┐
  │                                                          │
  ▼                                                          │
Keyboard Event ──► update.rs (handle_key_event) ──► TuiUiAction
                         │
                         ▼
                   TuiAppState (Mutated)
                         │
                         ▼
                  screens::chat::draw_chat_screen (Pure Views)
```

1. **State (`state.rs`)**: `TuiAppState` contains nested sub-states for each component (`ChromeState`, `ChatState`, `LineageState`, `DetailsState`, `DiagnosticsState`, `SessionSwitcherState`, `ApprovalModalState`, `OnboardingState`, `NotificationState`). The state also tracks whether the session has started (`has_started_session()`) and exposes lifecycle helpers: `start_new_session()` resets to a fresh pending session, `show_notification()` queues a dismissible overlay message.
2. **Update (`update.rs`)**: Translates key events or background notifications into local state mutations and emits `TuiUiAction` enum tokens for any action requiring external I/O or loop control.
3. **Services (`services.rs`)**: Encapsulates all filesystem lookups (run lists, lineage summaries, config parsing) into read-only, plain-old-data structures (`SessionListModel` and `LineageTreeModel`) that views digest.
4. **Pure Rendering (`widgets/` & `screens/`)**: Rendering paths accept read-only states or pre-loaded models and draw directly to the terminal frame. They are guaranteed to be free of file lookups, locking, channels, or network queries.

---

## 2. Dynamic Viewport & Screen Layout

The TUI dynamic layout compositor divides the viewport into structured blocks:

- **Top Panel Area**: Takes all remaining vertical space. Displays:
  - **Lineage Tree Sidebar**: Collapsible panel on the left (25% width).
  - **Chat Area**: Centered panel (takes remaining or full space).
  - **Drawers (Details & Diagnostics)**: Collapsible panel on the right (45% width). If both are open, splits the drawer area vertically 50/50.
- **Status Bar**: 3 rows high, displays the active status, session title, system mode, lineage hint, and contextual help hints. Long status messages are truncated to prevent layout overflow. Error details are reserved for the notification popup overlay.
- **Prompt Input**: 3 rows high at the bottom, displays text input buffer, slash-command autocomplete, and cursor. The prompt title reflects session state: `NEW SESSION MODE` for a fresh unused session, `EXPLICIT BRANCH MODE` when using `/branch`, and `CONTINUE CHAT MODE` when resuming or switching into an existing session. Expands to 12 rows during slash-command input to show the autocomplete list.

### Viewport Width Policy (R5)
To ensure readability, if the terminal width is less than 80 columns:
1. The **Lineage Tree Sidebar** is automatically hidden, even if explicitly toggled open.
2. The sidebar cannot be focused.
3. The Status Bar replaces the lineage toggle hint with: `Tab: lineage unavailable in narrow view`.

---

## 3. Keyboard Mapping & Focus Priorities

Keyboard input is routed strictly according to the active focus context:

### Active Modals (Overlay Priority)
If an overlay is active, all input is captured by the modal:
- **Help Modal (`F1` / `Ctrl+H`)**: Pressing any key closes the help guide.
- **Session Switcher (`F2` / `Ctrl+S`)**:
  - `Up`/`Down`: Move selection highlight index.
  - `Enter`: Selects and switches active session (triggers lineage tree load).
  - `Esc`: Closes switcher.
- **Tool Approval Modal**:
  - `a` / `y`: Approves tool execution.
  - `d` / `n` / `c`: Denies tool execution.
  - `Esc`: Denies tool execution.
- **Notification Modal**: Displays non-blocking error or warning messages (e.g. run failure details). Pressing any key or `Esc` dismisses the notification and returns to the previous focus context.
- **Onboarding Wizard**: Guides the user through provider selection and API key entry on first launch when no provider is configured. `Esc` quits, `q` quits, `Tab` toggles focus, `Enter` proceeds.

### Focus Cycling & Drawers Navigation
When no modal is active, the following hotkeys toggle collapsible views:
- `Tab`: Toggles Lineage Tree Sidebar (only if viewport width >= 80). Focus shifts to `LineageTree` on open.
- `F3` / `Ctrl+O`: Toggles Details (Config) Drawer. Focus shifts to `Details` on open.
- `F4` / `Ctrl+L`: Toggles Diagnostics (Tracing Logs) Drawer. Focus shifts to `Diagnostics` on open.
- `Esc`: Resets active focus back to `ChatPrompt`.

### Slash Commands

Typing `/` in the chat prompt activates autocomplete. The following commands are dispatched locally (not sent to the model):

| Command | Action |
|---|---|
| `/help` | Opens the keyboard shortcuts help modal |
| `/new` | Resets into a fresh pending session |
| `/quit` / `/exit` | Exits the TUI |
| `/mode <mode>` | Changes execution mode (confirm, yolo, human, dry-run, replay) |
| `/cost` | Displays the aggregated session cost |
| `/context` | Explains the context pipeline of the latest run |
| `/runs` | Opens the lineage tree sidebar (width >= 80) |
| `/branch <prompt>` | Branches the session from the selected or latest run |
| `/config` | Toggles the configuration/details drawer |
| `/export <format>` | Exports the latest run's trace (markdown, jsonl, sharegpt) |
| `/verify` | Runs verifiers on the latest run's artifacts |

Autocomplete navigation uses `Up`/`Down` arrows and `Tab` to complete the selected match.

### Focus-Specific Inputs
- **`ChatPrompt`**:
  - `Backspace`: Deletes characters.
  - `Enter`: Submits prompt to start or branch the session.
  - `Up`/`Down`: Scroll chat history list up/down.
- **`LineageTree`**:
  - `Up`/`Down`: Navigate runs tree. Setting selection targets the parent run ID for future branching commands.
  - `Left`: Return focus to `ChatPrompt`.
- **`Details`**:
  - `Up`/`Down`: Scroll configuration lines.
  - `Left`: Return focus to `ChatPrompt`.
- **`Diagnostics`**:
  - `Up`/`Down`: Scroll logging streams.
  - `Left`: Return focus to `ChatPrompt`.
