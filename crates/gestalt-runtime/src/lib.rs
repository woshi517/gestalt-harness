//! Gestalt runtime composition and embedding.
//!
//! [`api::v1`] is the sole stable v0.1 surface. [`unstable`] contains
//! first-party implementation APIs with no v0.1 compatibility guarantee.
//!
//! Crate-root imports are deliberately unavailable:
//!
//! ```compile_fail
//! use gestalt_runtime::AgentRuntimeBuilder;
//! ```

// Some crate-visible items live in private modules and are selectively exposed
// through `unstable`; changing them to `pub` would widen that surface.
#![allow(clippy::redundant_pub_crate)]

pub mod api;
pub mod unstable;

mod activation;
mod artifact_store;
mod builder;
mod compaction;
mod composition_hooks;
mod config;
mod context;
mod control;
mod discovery;
mod error;
mod event_bus;
mod exec;
mod extension;
mod extension_trust;
mod inspect;
mod jsonrpc;
mod lifecycle;
mod manifest;
mod orchestration;
mod permissions;
mod policy;
mod process_extension;
mod registry;
mod runtime;
mod session_queue;
mod tool_catalog;
mod tool_catalog_planner;
mod tool_output;
mod workspace_context;
mod workspace_snapshot;

#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "providers")]
mod providers;
#[cfg(feature = "skills")]
mod skills;
#[cfg(feature = "tools")]
mod tools;
#[cfg(feature = "trace")]
mod trace;
#[cfg(feature = "verify")]
mod verify;

// Temporary crate-private aliases keep implementation modules independent from
// the public export layout. They are not reachable by downstream crates.
#[allow(ambiguous_glob_reexports, unused_imports)]
mod internal_aliases {
    pub(crate) use crate::context::assembler::{
        estimate_message_tokens, estimate_text_tokens, ContextMessageAssembler,
    };
    pub(crate) use crate::exec::{
        ExecRequest, ExecResult, ExecutionSandbox, NoSandbox, SandboxMount,
    };
    #[cfg(feature = "mcp")]
    pub(crate) use crate::mcp::*;
    pub(crate) use crate::policy::engine::{
        classify_bash, BashPolicy, MinimalPolicyEngine, NetworkPolicy, PathPolicy, PolicyAction,
        PolicyConfig,
    };
    #[cfg(feature = "providers")]
    pub(crate) use crate::providers::*;
    #[cfg(feature = "skills")]
    pub(crate) use crate::skills::*;
    #[cfg(feature = "tools")]
    pub(crate) use crate::tools::*;
    #[cfg(feature = "trace")]
    pub(crate) use crate::trace::*;
    #[cfg(feature = "verify")]
    pub(crate) use crate::verify::*;

    pub(crate) use crate::context::{
        accounting, checkpoint_validation, compaction as context_compaction, default_prompt,
        tool_clearing, tool_exchanges,
    };
    #[cfg(feature = "mcp")]
    pub(crate) use crate::mcp::{
        client, error as mcp_error, model as mcp_model, registry as mcp_registry, transport,
    };
    #[cfg(feature = "providers")]
    pub(crate) use crate::providers::{
        auth, catalog, openai, registry as model_registry, sse, strict_schema,
    };
    #[cfg(feature = "skills")]
    pub(crate) use crate::skills::{
        activation as skill_activation, discovery as skill_discovery, events as skill_events,
        index as skill_index, manifest as skill_manifest, policy as skill_policy,
        resources as skill_resources,
    };
    #[cfg(feature = "tools")]
    pub(crate) use crate::tools::{backends, path, registry as tool_registry_module};
    #[cfg(feature = "trace")]
    pub(crate) use crate::trace::{
        context_artifacts, evaluator, fixture, golden, resume, run_manifest, tool_metrics,
    };
    #[cfg(feature = "verify")]
    pub(crate) use crate::verify::verifiers;

    pub(crate) use crate::activation::*;
    pub(crate) use crate::artifact_store::*;
    pub(crate) use crate::builder::*;
    pub(crate) use crate::composition_hooks::*;
    pub(crate) use crate::config::*;
    pub(crate) use crate::context::projection::*;
    pub(crate) use crate::context::report::*;
    pub(crate) use crate::context::*;
    pub(crate) use crate::control::*;
    pub(crate) use crate::discovery::*;
    pub(crate) use crate::error::*;
    pub(crate) use crate::event_bus::*;
    pub(crate) use crate::extension::*;
    pub(crate) use crate::extension_trust::*;
    pub(crate) use crate::inspect::*;
    pub(crate) use crate::manifest::*;
    pub(crate) use crate::orchestration::*;
    pub(crate) use crate::permissions::*;
    pub(crate) use crate::policy::*;
    pub(crate) use crate::registry::*;
    pub(crate) use crate::runtime::*;
    pub(crate) use crate::session_queue::*;
    pub(crate) use crate::tool_catalog::*;
    pub(crate) use crate::tool_catalog_planner::*;
    pub(crate) use crate::tool_output::*;
    pub(crate) use crate::workspace_context::*;
    pub(crate) use crate::workspace_snapshot::*;
}

#[allow(clippy::wildcard_imports, unused_imports)]
pub(crate) use internal_aliases::*;
