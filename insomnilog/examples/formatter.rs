//! Demonstrates `PatternFormatter` with several common logging styles.

use std::{error::Error, sync::Arc, thread, time::Duration};

use insomnilog::{
    BackendOptions, ConsoleSink, LogLevel, PatternFormatter, Sink, create_logger, log_info,
    log_warn, start,
};

/// Spins calling `pred` until it returns `true` or `timeout` elapses.
fn spin_until(pred: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        thread::yield_now();
    }
    false
}

/// Replaces every ASCII digit with `'x'` so timestamps and line numbers can be
/// matched without knowing their exact value. Also normalises the source file
/// path and module name to fixed tokens so the assertion holds regardless of
/// the compilation context (e.g. doctests use an auto-generated module path).
fn normalize(s: &str) -> String {
    s.replace("examples/examples/formatter.rs", "<file>")
        .replace(file!(), "<file>")
        .replace("formatter", "<module>")
        .replace(module_path!(), "<module>")
        .chars()
        .map(|c| if c.is_ascii_digit() { 'x' } else { c })
        .collect()
}

/// Creates a console sink and a capture sink from `pattern`, logs two records,
/// and asserts the normalized capture output equals `expected` when non-empty.
fn log_example(
    name: &str,
    pattern: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let console = Arc::new(ConsoleSink::new(
        PatternFormatter::new(pattern)?,
        LogLevel::Trace,
    ));
    let capture = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new(pattern)?,
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let logger = create_logger(
        name,
        vec![
            console as Arc<dyn Sink>,
            Arc::clone(&capture) as Arc<dyn Sink>,
        ],
        LogLevel::Trace,
    )?;

    // ANCHOR: message
    log_info!(
        logger,
        "The Answer to the Ultimate Question of Life, the Universe, and Everything.: {}",
        42_u16
    );
    log_warn!(logger, "...What{}", "?");
    // ANCHOR_END: message

    if !expected.is_empty() {
        let normalized_expected = normalize(expected);
        let ready = spin_until(
            || {
                let bytes = capture.captured_output();
                let text = String::from_utf8(bytes).expect("sink output is valid UTF-8");
                normalize(&text).len() >= normalized_expected.len()
            },
            Duration::from_millis(100),
        );
        assert!(
            ready,
            "capture sink did not receive all records within 100 ms"
        );
        let actual =
            String::from_utf8(capture.captured_output()).expect("sink output is valid UTF-8");
        assert_eq!(normalize(&actual), normalized_expected);
    }
    Ok(())
}

#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{secs}, {millis:03} etc. are PatternFormatter placeholders, not Rust format args"
)]
fn main() -> Result<(), Box<dyn Error>> {
    let _guard = start(BackendOptions::default())?;

    // ANCHOR: definition
    let fmt = PatternFormatter::new("{level:7} {message}")?;
    let sink = Arc::new(ConsoleSink::new(fmt, LogLevel::Trace));
    // ANCHOR_END: definition

    let _ = sink;

    // ANCHOR: minimal
    let pattern = "{level:7} {message}";
    let expected = concat!(
        "INFO    The Answer to the Ultimate Question of Life, the Universe, and Everything.: 42\n",
        "WARNING ...What?\n",
    );
    // ANCHOR_END: minimal
    log_example("minimal", pattern, expected)?;

    // ANCHOR: structured
    let pattern = "[{secs}.{millis:03}] {level:<8} {logger:<12} {message}";
    let expected = concat!(
        "[1779463945.562] INFO     Example 1    The Answer to the Ultimate Question of Life, the Universe, and Everything.: 42\n",
        "[1779463945.562] WARNING  Example 1    ...What?\n",
    );
    // ANCHOR_END: structured
    log_example("Example 1", pattern, expected)?;

    // ANCHOR: module
    let pattern = "{level} [{module}] {file}:{line} {message}";
    let expected = concat!(
        "INFO [formatter] examples/examples/formatter.rs:54 The Answer to the Ultimate Question of Life, the Universe, and Everything.: 42\n",
        "WARNING [formatter] examples/examples/formatter.rs:59 ...What?\n",
    );
    // ANCHOR_END: module
    log_example("Example 2", pattern, expected)?;

    // ANCHOR: centered
    let pattern = "{message:-^40}";
    let expected = concat!(
        "The Answer to the Ultimate Question of Life, the Universe, and Everything.: 42\n",
        "----------------...What?----------------\n",
    );
    // ANCHOR_END: centered
    log_example("Example 3", pattern, expected)?;

    Ok(())
}
