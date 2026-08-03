use std::sync::Arc;

use crate::{
    domain::event::EventEnvelope,
    error::{LoreError, Result},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    Inserted,
    Duplicate,
}

pub trait EventStore: Send + Sync {
    fn append_event(&self, event: &EventEnvelope) -> Result<AppendOutcome>;
    fn pending_event_count(&self) -> Result<u64>;
}

#[derive(Clone)]
pub struct CaptureService {
    store: Arc<dyn EventStore>,
}

impl CaptureService {
    pub fn new(store: Arc<dyn EventStore>) -> Self {
        Self { store }
    }

    pub fn ingest(&self, event: &EventEnvelope) -> Result<AppendOutcome> {
        event.validate().map_err(LoreError::Configuration)?;
        self.store.append_event(event)
    }

    pub fn pending_event_count(&self) -> Result<u64> {
        self.store.pending_event_count()
    }
}
