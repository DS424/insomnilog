//! Demonstrates eager per-thread queue allocation with `preallocate_thread`.

use std::{error::Error, sync::Arc, thread};

use insomnilog::{
    BackendOptions, ConsoleSink, LogLevel, PatternFormatter, create_logger, log_info,
    preallocate_thread, start,
};

fn main() -> Result<(), Box<dyn Error>> {
    let _guard = start(BackendOptions::default())?;

    let sink: Arc<dyn insomnilog::Sink> = Arc::new(ConsoleSink::new(
        PatternFormatter::default(),
        LogLevel::Info,
    ));
    let logger = create_logger("app", vec![sink], LogLevel::Info)?;

    // ANCHOR: main_thread
    // Allocate the queue for the main thread before entering the hot path.
    preallocate_thread();
    // ANCHOR_END: main_thread

    log_info!(logger, "main thread ready");

    // ANCHOR: worker_thread
    let worker_logger = Arc::clone(&logger);
    thread::spawn(move || {
        // Allocate the queue for this thread before any log call.
        preallocate_thread();

        log_info!(worker_logger, "worker thread ready");
    })
    .join()
    .unwrap();
    // ANCHOR_END: worker_thread

    Ok(())
}
