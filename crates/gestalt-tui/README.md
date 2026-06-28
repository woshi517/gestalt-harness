# Gestalt TUI Crate (`gestalt-tui`)

Standalone terminal UI binary for gestalt-harness.

This crate provides the `gestalt-tui` executable and the TUI presentation layer. It depends on `gestalt-app` for shared product services and `gestalt-runtime` for the underlying runtime stack.

Bare `gestalt` delegates to this binary by default when it is installed.
