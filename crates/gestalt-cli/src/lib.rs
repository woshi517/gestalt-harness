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
pub mod chat;
pub mod cost;
pub mod export;
pub mod output;
pub mod policy;
pub mod replay;
pub mod slash;
pub mod tools;
pub mod trace;
pub mod runs;
