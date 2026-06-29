# Gestalt App Crate (`gestalt-app`)

Reusable product services built on top of `gestalt-runtime`.

This crate owns config loading, workspace/report models, runtime factory wiring, run/session services, and other logic shared by both user-facing binaries.

`gestalt-app` is not a standalone binary. `gestalt-cli` and `gestalt-tui` call into it for product behavior that should not live in either UI shell.
