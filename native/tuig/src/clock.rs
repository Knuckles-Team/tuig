//! Injected time source for collaborators that compare against an expiry.
//!
//! Production reads the system clock; tests supply a controlled source so
//! expiry behavior never depends on wall time or a sleep.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> u64 + Send + Sync>);

impl Clock {
    pub fn system() -> Self {
        Self::from_fn(system_now_ms)
    }

    pub fn from_fn(source: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(source))
    }

    pub fn now_ms(&self) -> u64 {
        (self.0)()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::system()
    }
}

/// A broken system clock reads as the far future, so every expiry fails closed.
fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(u64::MAX)
}
