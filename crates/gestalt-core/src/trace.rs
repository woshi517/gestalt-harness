use crate::{error::TraceError, event::AgentEvent};

pub trait TraceSink: Send + Sync {
    fn emit(&self, event: AgentEvent) -> Result<(), TraceError>;
    fn flush(&self) -> Result<(), TraceError>;
    fn update_snapshot(&self, _snapshot: crate::snapshot::WorkspaceSnapshot) {}
}

#[derive(Debug, Default)]
pub struct NullTraceSink;

impl TraceSink for NullTraceSink {
    fn emit(&self, _event: AgentEvent) -> Result<(), TraceError> {
        Ok(())
    }

    fn flush(&self) -> Result<(), TraceError> {
        Ok(())
    }
}
