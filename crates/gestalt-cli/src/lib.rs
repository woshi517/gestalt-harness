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

pub mod approval;
pub mod auth;
pub mod chat;
pub mod config;
pub mod connect;
pub mod context;
pub mod cost;
pub mod doctor;
pub mod export;
pub mod model_cache;
pub mod models;
pub mod output;
pub mod policy;
pub mod profiles;
pub mod provider_catalog;
pub mod providers;
pub mod replay;
pub mod run;
pub mod runs;
pub mod runtime;
pub mod sessions;
pub mod slash;
pub mod tools;
pub mod trace;
pub mod verify;
pub mod workspace;

#[cfg(feature = "tui")]
pub mod tui;
