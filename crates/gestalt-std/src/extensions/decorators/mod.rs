#![forbid(unsafe_code)]
//! Composable higher-order [`gestalt_kernel::LoopDecorator`] implementations (`goal_check`,
//! `compaction`, `curator`) that wrap any `Box<dyn AgentLoop>`.

pub mod compaction;
pub mod curator;
pub mod goal_check;

pub use compaction::CompactionDecorator;
pub use curator::CuratorDecorator;
pub use goal_check::GoalCheckDecorator;
