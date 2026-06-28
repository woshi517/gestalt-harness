#![deny(clippy::all, clippy::pedantic)]
#![allow(
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::collapsible_match,
    clippy::default_trait_access,
    clippy::useless_vec,
    clippy::unused_async,
    clippy::uninlined_format_args,
    clippy::too_long_first_doc_paragraph,
    clippy::if_not_else,
    clippy::cast_precision_loss,
    clippy::redundant_closure,
    clippy::map_unwrap_or,
    clippy::items_after_statements,
    clippy::missing_panics_doc,
    clippy::too_many_arguments,
    clippy::redundant_closure_for_method_calls,
    clippy::match_same_arms,
    clippy::or_fun_call,
    clippy::struct_excessive_bools,
    clippy::implicit_clone,
    clippy::must_use_candidate,
    clippy::ignored_unit_patterns,
    clippy::manual_let_else,
    clippy::useless_let_if_seq,
    clippy::large_futures
)]

pub extern crate gestalt_runtime as gestalt_models;
pub extern crate gestalt_runtime as gestalt_policy;
pub extern crate gestalt_runtime as gestalt_tools;
pub extern crate gestalt_runtime as gestalt_trace;

pub mod auth {
    pub use gestalt_app::auth::resolve_auth;
}

pub mod config {
    pub use gestalt_app::config::{
        load_effective_config, mode_from_str, validate_workspace_config, CliOverrides,
        EffectiveConfig,
    };
}

pub mod connect {
    pub use gestalt_app::connect::connect_provider;
}

#[path = "../../gestalt-cli/src/cost.rs"]
pub mod cost;

pub mod context {
    pub use gestalt_app::context::explain_context;
}

#[path = "../../gestalt-cli/src/export.rs"]
pub mod export;

#[path = "../../gestalt-cli/src/output.rs"]
pub mod output;

pub mod run {
    pub use gestalt_app::run::run_prompt;
}

pub mod runs {
    pub use gestalt_app::runs::resolve_run_path;
}

pub mod sessions {
    pub use gestalt_app::reports::{RunManifestSummary, SessionSummary};
    pub use gestalt_app::sessions::*;
}

#[path = "../../gestalt-cli/src/replay.rs"]
pub mod replay;

#[path = "../../gestalt-cli/src/slash.rs"]
pub mod slash;

#[path = "../../gestalt-cli/src/trace.rs"]
pub mod trace;

pub mod verify {
    pub use gestalt_app::verify::verify_run;
}

#[path = "../../gestalt-cli/src/tui/mod.rs"]
pub mod tui;

pub use tui::run_tui;
