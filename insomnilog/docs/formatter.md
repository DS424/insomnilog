# Configuring the formatter

Some sinks, e.g. the `ConsoleSink`, use a `PatternFormatter` to turn a `LogRecord` into a line of text using a
configurable pattern string. A pattern is a mix of literal text and
`{field}` or `{field:spec}` placeholders that are substituted at format time.

```rust
{{#include ../examples/formatter.rs:definition}}
```

## Available Fields

| Field | Description | Example value |
| --- | --- | --- |
| `level` | Log level | `INFO` |
| `secs` | Whole seconds of the timestamp (seconds since the Unix epoch) | `1234567890` |
| `millis` | Millisecond component of the timestamp (0–999) | `042` |
| `file` | Source file path | `src/server.rs` |
| `line` | Source line number | `42` |
| `module` | Rust module path of the call site | `my_app::server` |
| `logger` | Name of the logger that emitted the record | `payments` |
| `message` | Fully formatted log message | `order #4291 confirmed` |

## Format spec

A placeholder can carry an optional format spec after a colon:
`{field:spec}`. The spec controls padding and alignment.

| Spec element | Syntax | Description |
| --- | --- | --- |
| Align left | `<` | Pad on the right (text flush left) |
| Align right | `>` | Pad on the left (text flush right, default for numeric fields) |
| Align center | `^` | Pad equally on both sides |
| Fill character | Any char before `<`, `>`, or `^` | Character used for padding (default: space) |
| Zero-pad | `0` before width | Use `0` as fill and force right-alignment |
| Width | Integer | Minimum output width |

Examples: `{level:<8}` left-aligns level in an 8-character field;
`{millis:03}` zero-pads the milliseconds to 3 digits; `{logger:->12}` right-aligns
the logger name with `-` fill in a 12-character field.

## Style examples

The following configurations showcase the output for different formatter configurations
of the following log messages: 

```rust
{{#include ../examples/formatter.rs:message}}
```

### Minimal — level and message only

```rust
{{#include ../examples/formatter.rs:minimal}}
```

### Structured — timestamp, aligned level, logger name

```rust
{{#include ../examples/formatter.rs:structured}}
```

### Module-style — level, module path, source location

```rust
{{#include ../examples/formatter.rs:module}}
```

### Center-aligned with fill — message only

```rust
{{#include ../examples/formatter.rs:centered}}
```

<details>
<summary>Full example</summary>

```rust
{{#include ../examples/formatter.rs}}
```

</details>

