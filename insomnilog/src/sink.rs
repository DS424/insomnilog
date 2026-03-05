//! Output sinks for formatted log records.

use std::io::{self, BufWriter, Stdout, Write};

/// A destination for formatted log output.
pub trait Sink {
    /// Writes a pre-formatted line (including newline) to the sink.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the write fails.
    fn write_line(&mut self, line: &str) -> io::Result<()>;

    /// Flushes any buffered output.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if flushing fails.
    fn flush(&mut self) -> io::Result<()>;
}

/// Sink that writes to standard output via a `BufWriter`.
pub struct ConsoleSink {
    /// Buffered writer for stdout.
    writer: BufWriter<Stdout>,
}

impl ConsoleSink {
    /// Creates a new console sink.
    pub fn new() -> Self {
        Self {
            writer: BufWriter::new(io::stdout()),
        }
    }
}

impl Sink for ConsoleSink {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
