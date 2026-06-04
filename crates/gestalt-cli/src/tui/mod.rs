#![allow(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::uninlined_format_args,
    clippy::too_many_lines,
    clippy::manual_split_once,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::option_if_let_else,
    clippy::use_self,
    clippy::useless_let_if_seq,
    clippy::items_after_statements,
    clippy::assigning_clones,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::collapsible_match
)]

pub mod app;
pub mod approval;
pub mod bridge;
pub mod screens;
pub mod services;
pub mod state;
pub mod update;
pub mod widgets;

pub use app::run_tui;
