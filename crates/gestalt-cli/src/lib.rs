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

#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
pub mod approval;
#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
pub mod chat;
#[cfg(all(feature = "providers", feature = "trace"))]
pub mod cost;
#[cfg(feature = "trace")]
pub mod export;
#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
pub mod output;
#[cfg(feature = "tools")]
pub mod policy;
#[cfg(feature = "trace")]
pub mod replay;
#[cfg(feature = "trace")]
pub mod runs;
#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
pub mod slash;
#[cfg(feature = "tools")]
pub mod tools;
#[cfg(all(feature = "providers", feature = "trace"))]
pub mod trace;
