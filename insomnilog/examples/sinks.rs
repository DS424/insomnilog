//! Runnable tour of every sink `insomnilog` ships with.
//!
//! One logger fans the same records out to all of them at once, so a single
//! run produces the console output *and* every file variant side by side:
//!
//! ```text
//! cargo run --example sinks
//! ```
//!
//! Run it two or three times in a row and compare the files it leaves in the
//! current directory — that is where the sinks differ:
//!
//! - `continuous.log` — keeps growing; every run is appended to the end.
//! - `session_latest.log` — only ever holds the newest run.
//! - `session_history.log` — holds the newest run; the three runs before it
//!   are in `session_history.log.1` … `.3`.

use std::{error::Error, sync::Arc, thread, time::Duration};

use insomnilog::{
    BackendOptions, ConsoleSink, ContinuousFileSink, LogLevel, NullSink, PatternFormatter,
    SessionFileSink, Sink, StreamSink, create_logger, log_info, start,
};

/// Number of counter ticks before the example shuts down on its own.
const TICKS: u32 = 10;

/// Pause between two counter ticks.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<(), Box<dyn Error>> {
    let guard = start(BackendOptions::default())?;

    // ANCHOR: console
    // Human-readable lines on stdout — what you watch while the program runs.
    let console = Arc::new(ConsoleSink::new(
        PatternFormatter::default(),
        LogLevel::Trace,
    ));
    // ANCHOR_END: console

    // ANCHOR: continuous
    // One file, appended to forever. Restarting the program continues the
    // same file instead of starting a new one.
    let continuous = Arc::new(ContinuousFileSink::try_new(
        PatternFormatter::default(),
        LogLevel::Trace,
        "continuous.log",
    )?);
    // ANCHOR_END: continuous

    // ANCHOR: overwrite
    // One file, replaced on every run: `max_backups = 0` keeps no history, so
    // this file always holds the latest run and nothing else.
    let latest = Arc::new(SessionFileSink::try_new(
        PatternFormatter::default(),
        LogLevel::Trace,
        "session_latest.log",
        0,
    )?);
    // ANCHOR_END: overwrite

    // ANCHOR: history
    // One file per run, with the three previous runs kept alongside it as
    // `session_history.log.1` … `.3`. The fourth-oldest is deleted.
    let history = Arc::new(SessionFileSink::try_new(
        PatternFormatter::default(),
        LogLevel::Trace,
        "session_history.log",
        3,
    )?);
    // ANCHOR_END: history

    // ANCHOR: advanced
    // `StreamSink` writes to any `std::io::Write` — here an in-memory buffer.
    let memory = Arc::new(StreamSink::new(
        PatternFormatter::default(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    // `NullSink` accepts every record and discards it.
    let discard = Arc::new(NullSink::new(LogLevel::Trace));
    // ANCHOR_END: advanced

    // ANCHOR: logger
    // A logger delivers each record to every sink it was created with.
    let logger = create_logger(
        "session",
        vec![
            console as Arc<dyn Sink>,
            continuous as Arc<dyn Sink>,
            latest as Arc<dyn Sink>,
            history as Arc<dyn Sink>,
            Arc::clone(&memory) as Arc<dyn Sink>,
            discard as Arc<dyn Sink>,
        ],
        LogLevel::Trace,
    )?;
    // ANCHOR_END: logger

    // ANCHOR: loop
    log_info!(logger, "Session started");

    for tick in 1..=TICKS {
        log_info!(logger, "Tick {}", tick);
        thread::sleep(TICK_INTERVAL);
    }

    log_info!(logger, "Session shutting down");
    // ANCHOR_END: loop

    // Dropping the guard drains every pending record and flushes all sinks;
    // after this point the files — and the in-memory buffer — are complete.
    drop(guard);

    let captured = String::from_utf8(memory.captured_output())?;
    println!(
        "\n{} lines captured in memory by the StreamSink",
        captured.lines().count()
    );
    println!(
        "wrote continuous.log, session_latest.log, session_history.log — run again to compare"
    );

    Ok(())
}
