use std::sync::Arc;
use turso_core::{Clock, IO, MemoryIO, MonotonicInstant, WallClockInstant};

/// The browser supplies real JS clocks; std::time clocks are unavailable on
/// wasm32-unknown-unknown. File storage remains Turso's in-memory implementation.
pub(super) struct MemoryHost {
    files: MemoryIO,
    clock: Arc<dyn Clock + Send + Sync>,
}
impl MemoryHost {
    pub(super) fn new(clock: Arc<dyn Clock + Send + Sync>) -> Self {
        Self {
            files: MemoryIO::new(),
            clock,
        }
    }
}
impl Clock for MemoryHost {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        self.clock.current_time_monotonic()
    }
    fn current_time_wall_clock(&self) -> WallClockInstant {
        self.clock.current_time_wall_clock()
    }
}
impl IO for MemoryHost {
    fn open_file(
        &self,
        path: &str,
        flags: turso_core::OpenFlags,
        direct: bool,
    ) -> turso_core::Result<Arc<dyn turso_core::File>> {
        self.files.open_file(path, flags, direct)
    }
    fn remove_file(&self, path: &str) -> turso_core::Result<()> {
        self.files.remove_file(path)
    }
}
