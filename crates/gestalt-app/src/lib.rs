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

pub extern crate gestalt_runtime as gestalt_context;
pub extern crate gestalt_runtime as gestalt_mcp;
pub extern crate gestalt_runtime as gestalt_models;
pub extern crate gestalt_runtime as gestalt_policy;
pub extern crate gestalt_runtime as gestalt_skills;
pub extern crate gestalt_runtime as gestalt_tools;
pub extern crate gestalt_runtime as gestalt_trace;
pub extern crate gestalt_runtime as gestalt_verify;

pub mod auth;
pub mod catalog;
pub mod config;
pub mod connect;
pub mod context;
pub mod doctor;
pub mod model_cache;
pub mod models;
pub mod profiles;
pub mod providers;
pub mod reports;
pub mod run;
pub mod runs;
pub mod runtime_factory;
pub mod sessions;
pub mod verify;
pub mod workspace;

pub trait InteractionProvider: Send + Sync {
    fn prompt_password(&self, prompt: &str) -> Option<String>;
    fn confirm(&self, prompt: &str) -> bool;
}
