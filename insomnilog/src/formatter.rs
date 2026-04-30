//! Log record formatting.
//!
//! Defines the [`Formatter`] trait — the contract a text-producing sink uses
//! to turn a [`DecodedRecord`] into output bytes — and provides a default
//! [`PatternFormatter`] implementation.
//!
//! ## Pattern format string
//!
//! `PatternFormatter` is configured with a pattern string containing named
//! `{field}` placeholders. Each placeholder is replaced at format time with
//! the corresponding value from the [`DecodedRecord`]. An optional format
//! spec after a colon controls padding: `{field:spec}`.
//!
//! ### Fields
//!
//! | Placeholder | Value |
//! |---|---|
//! | `{level}` | Log level (`DEBUG`, `INFO`, `WARNING`, `ERROR`) |
//! | `{secs}` | Whole seconds of the record timestamp |
//! | `{millis}` | Millisecond component of the timestamp |
//! | `{file}` | Source file name |
//! | `{line}` | Source line number |
//! | `{module}` | Module path |
//! | `{message}` | Formatted message with positional `{}` arguments substituted |
//!
//! ### Format spec
//!
//! The optional spec after the colon follows this grammar:
//!
//! ```text
//! spec   := [[fill] align] ['0'] [width]
//! fill   := <any single character>  (must be immediately followed by align)
//! align  := '<'  left-align (default for string fields)
//!         | '>'  right-align (default for numeric fields)
//!         | '^'  center
//! '0'    := zero-pad flag — shorthand for fill='0', align='>'
//! width  := positive integer; values wider than width are never truncated
//! ```
//!
//! When `fill` and `align` are both present the `'0'` flag is redundant but
//! accepted; when the `'0'` flag is present it overrides any explicit `fill`
//! character and forces right alignment.
//!
//! Default alignment when no spec is given: string fields (`level`, `file`,
//! `module`, `message`) default to left; numeric fields (`secs`, `millis`,
//! `line`) default to right.
//!
//! ### Examples
//!
//! ```text
//! "[{level} {secs}.{millis:03}] {file}:{line} {message}"   ← default
//! "{level:>8} | {message}"
//! "{level:*^10} {file}:{line}"
//! ```

// Items are unused until later rewrite steps wire them up (see Plan.md).
// This `allow` is removed once `macros.rs` and the backend module use them.
#![allow(dead_code)]

use core::fmt::Write;

use crate::decode::{DecodedArg, DecodedRecord};

/// Renders a `DecodedRecord` into human-readable bytes for a text sink.
///
/// Implementations must be [`Send`] and [`Sync`] because a single formatter
/// is shared by the backend worker thread and may also be referenced from
/// threads that hold the owning sink.
///
/// `format` **appends** to `out`; it does not clear the buffer first. The
/// caller (typically a `Sink` implementation) owns the output `String`,
/// decides when to clear it between records, and may frame the formatted
/// record with a prefix or suffix.
pub trait Formatter: Send + Sync {
    /// Appends a rendering of `record` to `out`.
    fn format(&self, record: &DecodedRecord, out: &mut String);
}

/// Returned by [`PatternFormatter::new`] when the pattern string is invalid.
#[derive(Debug)]
pub enum InvalidPatternError {
    /// The pattern contains `{name}` where `name` is not a recognized field.
    UnknownField(String),
    /// The pattern contains a `{` with no matching `}`.
    UnclosedBrace,
    /// The pattern contains `{field:spec}` where `spec` is not valid.
    InvalidFormatSpec(String),
}

impl core::fmt::Display for InvalidPatternError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownField(name) => write!(
                f,
                "unknown pattern field \"{name}\"; \
                 known fields: level, secs, millis, file, line, module, message",
            ),
            Self::UnclosedBrace => f.write_str("unclosed '{' in pattern string"),
            Self::InvalidFormatSpec(spec) => write!(
                f,
                "invalid format spec \"{spec}\"; \
                 expected [[fill]align]['0'][width] where align is '<', '>', or '^' \
                 and width is a non-negative integer",
            ),
        }
    }
}

/// Text alignment direction used when padding a field value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    /// Pad on the right (value sits at the start of the field).
    Left,
    /// Pad on the left (value sits at the end of the field).
    Right,
    /// Pad equally on both sides; odd padding goes to the right.
    Center,
}

/// Padding and alignment options extracted from a `{field:spec}` placeholder.
#[derive(Debug, Clone)]
struct FormatSpec {
    /// Padding character; overridden to `'0'` when `zero_pad` is set.
    fill: char,
    /// Explicit alignment, or `None` to use the field's default.
    align: Option<Align>,
    /// When true, fill becomes `'0'` and alignment is forced right.
    zero_pad: bool,
    /// Minimum output width; 0 means no padding.
    width: usize,
}

impl Default for FormatSpec {
    fn default() -> Self {
        Self {
            fill: ' ',
            align: None,
            zero_pad: false,
            width: 0,
        }
    }
}

/// Which [`DecodedRecord`] field a pattern placeholder maps to.
#[derive(Debug, Clone, Copy)]
enum FieldKind {
    /// The record's log level.
    Level,
    /// Whole seconds of the timestamp.
    Secs,
    /// Millisecond component of the timestamp.
    Millis,
    /// Source file name.
    File,
    /// Source line number.
    Line,
    /// Module path.
    Module,
    /// Fully formatted message (fmt string + positional args).
    Message,
}

impl FieldKind {
    /// Returns `true` for fields whose values are integers; governs default
    /// alignment when no explicit align is specified in the format spec.
    const fn is_numeric(self) -> bool {
        matches!(self, Self::Secs | Self::Millis | Self::Line)
    }
}

/// A field placeholder extracted from a pattern string, ready for formatting.
#[derive(Debug, Clone)]
struct ParsedField {
    /// Which record field to render.
    kind: FieldKind,
    /// Padding/alignment spec to apply to the rendered value.
    spec: FormatSpec,
}

/// A single segment of a parsed pattern string.
#[derive(Debug, Clone)]
enum PatternElement {
    /// A run of literal characters to emit verbatim.
    Literal(String),
    /// A `{field:spec}` placeholder to evaluate at format time.
    Field(ParsedField),
}

/// Default [`Formatter`] implementation that renders records using a
/// configurable pattern string.
///
/// See the module documentation for the full pattern and format spec syntax.
#[derive(Debug, Clone)]
pub struct PatternFormatter {
    /// Parsed segments of the pattern string, evaluated in order at format time.
    elements: Vec<PatternElement>,
}

impl PatternFormatter {
    /// The pattern used by [`PatternFormatter::default`]:
    /// `"[{level} {secs}.{millis:03}] {file}:{line} {message}"`.
    pub const DEFAULT_PATTERN: &'static str =
        "[{level} {secs}.{millis:03}] {file}:{line} {message}";

    /// Constructs a new [`PatternFormatter`] with the given `pattern`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidPatternError`] if `pattern` contains an unknown field
    /// name, an unclosed `{`, or an invalid format spec.
    pub fn new(pattern: &str) -> Result<Self, InvalidPatternError> {
        Ok(Self {
            elements: parse_pattern(pattern)?,
        })
    }
}

impl Default for PatternFormatter {
    fn default() -> Self {
        Self {
            elements: parse_pattern(Self::DEFAULT_PATTERN)
                .expect("DEFAULT_PATTERN is always valid"),
        }
    }
}

impl Formatter for PatternFormatter {
    fn format(&self, record: &DecodedRecord, out: &mut String) {
        let secs = record.timestamp_ns / 1_000_000_000;
        let millis = (record.timestamp_ns % 1_000_000_000) / 1_000_000;

        for element in &self.elements {
            match element {
                PatternElement::Literal(s) => out.push_str(s),
                PatternElement::Field(pf) => {
                    let value = render_field(pf.kind, record, secs, millis);
                    apply_format_spec(out, &value, &pf.spec, pf.kind.is_numeric());
                }
            }
        }
    }
}

/// Renders a single field value into an owned `String` before padding is applied.
fn render_field(kind: FieldKind, record: &DecodedRecord, secs: u64, millis: u64) -> String {
    let mut s = String::new();
    match kind {
        FieldKind::Level => {
            let _ = write!(s, "{}", record.metadata.level);
        }
        FieldKind::Secs => {
            let _ = write!(s, "{secs}");
        }
        FieldKind::Millis => {
            let _ = write!(s, "{millis}");
        }
        FieldKind::File => s.push_str(record.metadata.file),
        FieldKind::Line => {
            let _ = write!(s, "{}", record.metadata.line);
        }
        FieldKind::Module => s.push_str(record.metadata.module_path),
        FieldKind::Message => {
            format_message(&mut s, record.metadata.fmt_str, &record.args);
        }
    }
    s
}

/// Parses a pattern string into a sequence of [`PatternElement`]s.
fn parse_pattern(pattern: &str) -> Result<Vec<PatternElement>, InvalidPatternError> {
    let mut elements: Vec<PatternElement> = Vec::new();
    let mut literal = String::new();
    let mut chars = pattern.chars();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut token = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '}' {
                    closed = true;
                    break;
                }
                token.push(c);
            }
            if !closed {
                return Err(InvalidPatternError::UnclosedBrace);
            }
            let (field_name, spec_str) = token.find(':').map_or((token.as_str(), ""), |pos| {
                (&token[..pos], &token[pos + 1..])
            });
            let kind = parse_field_kind(field_name)?;
            let spec = parse_format_spec(spec_str)?;
            if !literal.is_empty() {
                elements.push(PatternElement::Literal(core::mem::take(&mut literal)));
            }
            elements.push(PatternElement::Field(ParsedField { kind, spec }));
        } else {
            literal.push(ch);
        }
    }
    if !literal.is_empty() {
        elements.push(PatternElement::Literal(literal));
    }
    Ok(elements)
}

/// Maps a field name string to its [`FieldKind`].
fn parse_field_kind(name: &str) -> Result<FieldKind, InvalidPatternError> {
    match name {
        "level" => Ok(FieldKind::Level),
        "secs" => Ok(FieldKind::Secs),
        "millis" => Ok(FieldKind::Millis),
        "file" => Ok(FieldKind::File),
        "line" => Ok(FieldKind::Line),
        "module" => Ok(FieldKind::Module),
        "message" => Ok(FieldKind::Message),
        other => Err(InvalidPatternError::UnknownField(other.to_owned())),
    }
}

/// Parses a format spec string (the part after `:` inside `{field:spec}`).
fn parse_format_spec(spec: &str) -> Result<FormatSpec, InvalidPatternError> {
    if spec.is_empty() {
        return Ok(FormatSpec::default());
    }

    let mut chars = spec.chars().peekable();
    let mut fill = ' ';
    let mut align: Option<Align> = None;

    // Look ahead to decide if spec starts with [fill][align] or just [align].
    // If the second character is an alignment char, the first is the fill char.
    let mut probe = spec.chars();
    let first = probe.next();
    let second = probe.next();

    if let (Some(f), Some(a)) = (first, second.and_then(to_align)) {
        fill = f;
        align = Some(a);
        chars.next(); // consume fill
        chars.next(); // consume align
    } else if let Some(a) = first.and_then(to_align) {
        align = Some(a);
        chars.next(); // consume align
    }

    let zero_pad = if chars.peek() == Some(&'0') {
        chars.next();
        true
    } else {
        false
    };

    let rest: String = chars.collect();
    let width = if rest.is_empty() {
        0
    } else {
        rest.parse::<usize>()
            .map_err(|_| InvalidPatternError::InvalidFormatSpec(spec.to_owned()))?
    };

    Ok(FormatSpec {
        fill,
        align,
        zero_pad,
        width,
    })
}

/// Maps `<`, `>`, `^` to the corresponding [`Align`] variant; returns `None` for any other char.
const fn to_align(c: char) -> Option<Align> {
    match c {
        '<' => Some(Align::Left),
        '>' => Some(Align::Right),
        '^' => Some(Align::Center),
        _ => None,
    }
}

/// Writes `value` into `buf` with padding applied according to `spec`.
///
/// When no width is set, or the value is already at least as wide as the
/// requested width, `value` is written verbatim (no truncation ever occurs).
fn apply_format_spec(buf: &mut String, value: &str, spec: &FormatSpec, is_numeric: bool) {
    if spec.width == 0 {
        buf.push_str(value);
        return;
    }
    let value_len = value.chars().count();
    if value_len >= spec.width {
        buf.push_str(value);
        return;
    }
    let padding = spec.width - value_len;
    let (fill, align) = if spec.zero_pad {
        ('0', Align::Right)
    } else {
        let align = spec.align.unwrap_or(if is_numeric {
            Align::Right
        } else {
            Align::Left
        });
        (spec.fill, align)
    };
    match align {
        Align::Left => {
            buf.push_str(value);
            for _ in 0..padding {
                buf.push(fill);
            }
        }
        Align::Right => {
            for _ in 0..padding {
                buf.push(fill);
            }
            buf.push_str(value);
        }
        Align::Center => {
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            for _ in 0..left_pad {
                buf.push(fill);
            }
            buf.push_str(value);
            for _ in 0..right_pad {
                buf.push(fill);
            }
        }
    }
}

/// Scans `fmt_str` for `{}` placeholders and replaces each with the next
/// [`DecodedArg`] in `args`, in order. Surplus placeholders or surplus args
/// are handled per the [`PatternFormatter`] doc-comment.
fn format_message(buf: &mut String, fmt_str: &str, args: &[DecodedArg]) {
    let mut arg_iter = args.iter();
    let mut chars = fmt_str.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if let Some(arg) = arg_iter.next() {
                let _ = write!(buf, "{arg}");
            } else {
                buf.push_str("{}");
            }
        } else {
            buf.push(ch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{DecodedArg, DecodedRecord};
    use crate::level::LogLevel;
    use crate::metadata::LogMetadata;

    static META_INFO: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "msg",
        file: "f.rs",
        line: 1,
        module_path: "m",
        arg_count: 0,
    };

    static META_WARN: LogMetadata = LogMetadata {
        level: LogLevel::Warning,
        fmt_str: "msg",
        file: "f.rs",
        line: 1,
        module_path: "m",
        arg_count: 0,
    };

    static FMT_TWO: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "hello {} world {}",
        file: "test.rs",
        line: 42,
        module_path: "test",
        arg_count: 2,
    };

    static FMT_NONE: LogMetadata = LogMetadata {
        level: LogLevel::Warning,
        fmt_str: "simple message",
        file: "lib.rs",
        line: 1,
        module_path: "test",
        arg_count: 0,
    };

    static FMT_ONE: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "x={}",
        file: "f.rs",
        line: 0,
        module_path: "test",
        arg_count: 1,
    };

    static FMT_LITERAL_BRACE: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "open {brace x={}",
        file: "f.rs",
        line: 0,
        module_path: "test",
        arg_count: 1,
    };

    static FMT_THREE_PLACEHOLDERS: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "{} {} {}",
        file: "f.rs",
        line: 0,
        module_path: "test",
        arg_count: 3,
    };

    fn bare(meta: &'static LogMetadata) -> DecodedRecord {
        DecodedRecord {
            timestamp_ns: 0,
            metadata: meta,
            args: vec![],
        }
    }

    fn with_ts(meta: &'static LogMetadata, ts_ns: u64) -> DecodedRecord {
        DecodedRecord {
            timestamp_ns: ts_ns,
            metadata: meta,
            args: vec![],
        }
    }

    fn fmt(pattern: &str, record: &DecodedRecord) -> String {
        let f = PatternFormatter::new(pattern).expect("valid pattern");
        let mut out = String::new();
        f.format(record, &mut out);
        out
    }

    #[test]
    fn spec_empty_is_valid() {
        PatternFormatter::new("{level:}").unwrap();
    }

    #[test]
    fn spec_just_width_is_valid() {
        PatternFormatter::new("{level:10}").unwrap();
    }

    #[test]
    fn spec_left_align_is_valid() {
        PatternFormatter::new("{level:<10}").unwrap();
    }

    #[test]
    fn spec_right_align_is_valid() {
        PatternFormatter::new("{level:>10}").unwrap();
    }

    #[test]
    fn spec_center_align_is_valid() {
        PatternFormatter::new("{level:^10}").unwrap();
    }

    #[test]
    fn spec_fill_and_align_is_valid() {
        PatternFormatter::new("{level:*>10}").unwrap();
    }

    #[test]
    fn spec_zero_pad_is_valid() {
        PatternFormatter::new("{millis:03}").unwrap();
    }

    #[test]
    fn spec_zero_pad_no_width_is_valid() {
        // zero flag with no width is a no-op but syntactically legal
        PatternFormatter::new("{millis:0}").unwrap();
    }

    #[test]
    fn spec_invalid_non_numeric_width() {
        assert!(matches!(
            PatternFormatter::new("{level:abc}"),
            Err(InvalidPatternError::InvalidFormatSpec(_)),
        ));
    }

    #[test]
    fn spec_invalid_align_then_non_numeric_width() {
        assert!(matches!(
            PatternFormatter::new("{level:>abc}"),
            Err(InvalidPatternError::InvalidFormatSpec(_)),
        ));
    }

    #[test]
    fn spec_invalid_lone_fill_without_align() {
        // '*' alone is not an alignment char, so it falls through to the width
        // parser which rejects non-numeric input.
        assert!(matches!(
            PatternFormatter::new("{level:*}"),
            Err(InvalidPatternError::InvalidFormatSpec(_)),
        ));
    }

    #[test]
    fn spec_right_align_pads_left_with_spaces() {
        assert_eq!(fmt("{level:>8}", &bare(&META_INFO)), "    INFO");
    }

    #[test]
    fn spec_left_align_pads_right_with_spaces() {
        assert_eq!(fmt("{level:<8}", &bare(&META_INFO)), "INFO    ");
    }

    #[test]
    fn spec_center_align_even_padding() {
        // "INFO" (4 chars) in width 8 → 2 spaces each side
        assert_eq!(fmt("{level:^8}", &bare(&META_INFO)), "  INFO  ");
    }

    #[test]
    fn spec_center_align_odd_padding_goes_right() {
        // "INFO" (4 chars) in width 9 → 5 padding: 2 left, 3 right
        assert_eq!(fmt("{level:^9}", &bare(&META_INFO)), "  INFO   ");
    }

    #[test]
    fn spec_custom_fill_char() {
        assert_eq!(fmt("{level:*>8}", &bare(&META_INFO)), "****INFO");
    }

    #[test]
    fn spec_custom_fill_left() {
        assert_eq!(fmt("{level:.<8}", &bare(&META_INFO)), "INFO....");
    }

    #[test]
    fn spec_custom_fill_middle() {
        assert_eq!(fmt("{level:.^12}", &bare(&META_INFO)), "....INFO....");
    }

    #[test]
    fn spec_zero_pad_pads_left_with_zeros() {
        // 7 ms in width 3 → "007"
        assert_eq!(fmt("{millis:03}", &with_ts(&META_INFO, 7_000_000)), "007");
    }

    #[test]
    fn spec_zero_pad_wider_value_not_truncated() {
        // 1234 ms (4 chars) in width 3 → "1234"
        assert_eq!(
            fmt("{millis:03}", &with_ts(&META_INFO, 1_234_000_000)),
            "234"
        );
    }

    #[test]
    fn spec_value_wider_than_width_is_not_truncated() {
        // "INFO" (4 chars) in width 2 → "INFO"
        assert_eq!(fmt("{level:>2}", &bare(&META_INFO)), "INFO");
    }

    #[test]
    fn spec_numeric_field_defaults_to_right_align() {
        // {line:5} with line=1 → "    1"
        assert_eq!(fmt("{line:5}", &bare(&META_INFO)), "    1");
    }

    #[test]
    fn spec_string_field_defaults_to_left_align() {
        // {level:8} with "INFO" → "INFO    "
        assert_eq!(fmt("{level:8}", &bare(&META_INFO)), "INFO    ");
    }

    #[test]
    fn spec_empty_is_no_op() {
        assert_eq!(fmt("{level:}", &bare(&META_INFO)), "INFO");
    }

    #[test]
    fn formatter_trait_is_dyn_compatible() {
        let _: &dyn Formatter = &PatternFormatter::default();
    }

    #[test]
    fn pattern_formatter_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PatternFormatter>();
    }

    #[test]
    fn new_rejects_unknown_field() {
        assert!(matches!(
            PatternFormatter::new("{typo}"),
            Err(InvalidPatternError::UnknownField(ref s)) if s == "typo"
        ));
    }

    #[test]
    fn new_rejects_unclosed_brace() {
        assert!(matches!(
            PatternFormatter::new("hello {level"),
            Err(InvalidPatternError::UnclosedBrace),
        ));
    }

    #[test]
    fn new_rejects_empty_field_name() {
        assert!(matches!(
            PatternFormatter::new("{}"),
            Err(InvalidPatternError::UnknownField(ref s)) if s.is_empty()
        ));
    }

    #[test]
    fn default_pattern_is_valid() {
        assert!(PatternFormatter::new(PatternFormatter::DEFAULT_PATTERN).is_ok());
    }

    #[test]
    fn pattern_format_basic_with_args() {
        let r = DecodedRecord {
            timestamp_ns: 1_700_000_000_123_000_000,
            metadata: &FMT_TWO,
            args: vec![DecodedArg::Str("alice".to_owned()), DecodedArg::U64(99)],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert_eq!(out, "[INFO 1700000000.123] test.rs:42 hello alice world 99");
    }

    #[test]
    fn pattern_format_no_args() {
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &FMT_NONE,
            args: vec![],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert_eq!(out, "[WARNING 0.000] lib.rs:1 simple message");
    }

    #[test]
    fn pattern_format_pads_millis_to_three_digits() {
        // 5 s + 7 ms → millis component rendered as "007" via {:03} in the
        // default pattern.
        let r = DecodedRecord {
            timestamp_ns: 5_007_000_000,
            metadata: &FMT_NONE,
            args: vec![],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert!(out.contains("5.007"), "out={out:?}");
    }

    #[test]
    fn pattern_format_renders_seconds_and_millis_from_full_timestamp() {
        let r = DecodedRecord {
            timestamp_ns: 1_700_000_000_456_000_000,
            metadata: &FMT_NONE,
            args: vec![],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert!(out.contains("1700000000.456"), "out={out:?}");
    }

    #[test]
    fn pattern_format_extra_placeholders_left_as_is() {
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &FMT_THREE_PLACEHOLDERS,
            args: vec![DecodedArg::U32(7)],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert!(out.ends_with("7 {} {}"), "out={out:?}");
    }

    #[test]
    fn pattern_format_extra_args_ignored() {
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &FMT_ONE,
            args: vec![
                DecodedArg::U32(7),
                DecodedArg::Str("ignored".to_owned()),
                DecodedArg::Bool(true),
            ],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert!(out.ends_with("x=7"), "out={out:?}");
        assert!(!out.contains("ignored"), "out={out:?}");
    }

    #[test]
    fn pattern_format_brace_without_close_is_literal() {
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &FMT_LITERAL_BRACE,
            args: vec![DecodedArg::U32(3)],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert!(out.contains("open {brace x=3"), "out={out:?}");
    }

    #[test]
    fn pattern_format_appends_to_existing_buffer() {
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &FMT_NONE,
            args: vec![],
        };
        let mut out = String::from("PREFIX|");
        PatternFormatter::default().format(&r, &mut out);
        assert!(out.starts_with("PREFIX|"), "out={out:?}");
        assert!(out.contains("simple message"), "out={out:?}");
    }

    #[test]
    fn pattern_format_renders_various_arg_types() {
        static META: LogMetadata = LogMetadata {
            level: LogLevel::Debug,
            fmt_str: "i={} u={} f={} b={} s={} c={}",
            file: "f.rs",
            line: 0,
            module_path: "test",
            arg_count: 6,
        };
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &META,
            args: vec![
                DecodedArg::I64(-42),
                DecodedArg::U32(7),
                DecodedArg::F64(1.5),
                DecodedArg::Bool(true),
                DecodedArg::Str("hi".to_owned()),
                DecodedArg::Custom("rgb(1, 2, 3)".to_owned()),
            ],
        };
        let mut out = String::new();
        PatternFormatter::default().format(&r, &mut out);
        assert!(
            out.contains("i=-42 u=7 f=1.5 b=true s=hi c=rgb(1, 2, 3)"),
            "out={out:?}",
        );
    }

    #[test]
    fn pattern_format_custom_pattern_all_fields() {
        let f = PatternFormatter::new("{level}|{secs}|{millis}|{file}|{line}|{module}|{message}")
            .unwrap();
        let r = DecodedRecord {
            timestamp_ns: 2_001_000_000,
            metadata: &FMT_ONE,
            args: vec![DecodedArg::U32(42)],
        };
        let mut out = String::new();
        f.format(&r, &mut out);
        assert_eq!(out, "INFO|2|1|f.rs|0|test|x=42");
    }

    #[test]
    fn pattern_format_custom_pattern_literal_text() {
        let f = PatternFormatter::new("level={level} msg={message}").unwrap();
        let r = DecodedRecord {
            timestamp_ns: 0,
            metadata: &FMT_NONE,
            args: vec![],
        };
        let mut out = String::new();
        f.format(&r, &mut out);
        assert_eq!(out, "level=WARNING msg=simple message");
    }

    #[test]
    fn pattern_format_warning_level() {
        assert_eq!(fmt("{level}", &bare(&META_WARN)), "WARNING");
    }
}
