//! Minimal quick-start example for `insomnilog`.

use std::{error::Error, sync::Arc};

use insomnilog::{
    ConsoleSink, LogLevel, PatternFormatter, log_debug, log_error, log_info, log_warn,
    preallocate_thread,
};

fn main() -> Result<(), Box<dyn Error>> {
    // 1. Start the backend worker thread. The returned guard keeps it alive;
    // dropping it drains all buffered records and joins the thread.
    let _guard = insomnilog::start(insomnilog::BackendOptions::default())?;

    // 2. Create a sink — decides where and how records are written. The built-in
    // `ConsoleSink` formats records as human-readable lines and writes them to
    // stdout.
    let sink = Arc::new(ConsoleSink::new(
        PatternFormatter::default(),
        LogLevel::Trace,
    ));

    // 3. Create a logger — the handle you pass to every log macro. It holds a
    // level filter and the list of sinks that receive its records.
    let logger = insomnilog::create_logger("app", vec![sink], LogLevel::Trace)?;

    // Optional: Eagerly allocate this thread's queue so the first log call is
    // allocation-free. Omit it and allocation happens on first use.
    preallocate_thread();

    // 4. Start logging
    // The macros accept a format string and zero or more typed arguments, similar to
    // `println!`
    log_info!(logger, "application started");
    log_debug!(logger, "config loaded from {}", "/etc/app/config.toml");
    log_warn!(logger, "disk usage at {}%", 87.5_f64);
    log_error!(logger, "connection lost to {}", "db-primary");

    // _guard drops here → backend drains and exits cleanly.
    Ok(())
}
