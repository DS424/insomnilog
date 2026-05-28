//! Code backing the "Core concepts" documentation chapter.

use std::{error::Error, sync::Arc};

use insomnilog::{
    BackendOptions, ConsoleSink, LogLevel, PatternFormatter, create_logger, log_info,
    register_sink, start,
};

fn main() -> Result<(), Box<dyn Error>> {
    // ANCHOR: backend
    let _guard = start(BackendOptions::default())?;
    // ANCHOR_END: backend

    // ANCHOR: sink
    let sink: Arc<dyn insomnilog::Sink> = Arc::new(ConsoleSink::new(
        PatternFormatter::default(),
        LogLevel::Trace,
    ));
    register_sink("console", Arc::clone(&sink))?;
    // ANCHOR_END: sink

    // ANCHOR: logger
    let logger = create_logger("app", vec![Arc::clone(&sink)], LogLevel::Info)?;

    log_info!(logger, "server started on port {}", 8080_u16);
    // ANCHOR_END: logger

    Ok(())
}
