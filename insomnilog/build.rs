#![allow(missing_docs, clippy::missing_docs_in_private_items)]
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("generated_docs.md");

    let mut out = resolve_includes("docs/about.md");
    out.push_str(&resolve_includes("docs/quick_start.md"));
    out.push_str(&resolve_includes("docs/core_concepts.md"));
    out.push_str(&resolve_includes("docs/architecture.md"));
    out.push_str(&resolve_includes("docs/preallocating.md"));
    out.push_str(&resolve_includes("docs/sinks.md"));
    out.push_str(&resolve_includes("docs/formatter.md"));
    out.push_str(&mermaid_script());
    std::fs::write(dest, out).unwrap();

    println!("cargo:rerun-if-changed=docs/");
    println!("cargo:rerun-if-changed=examples/");
}

fn mermaid_script() -> String {
    let js = std::fs::read_to_string("docs/mermaid.tiny.js")
        .expect("could not read docs/mermaid.tiny.js");

    // Rustdoc renders ```mermaid as <pre class="language-mermaid"><code>…</code></pre>
    // wrapped in <div class="example-wrap">. Replace those with <div class="mermaid">
    // so Mermaid.js can render them.
    //
    // Theme follows rustdoc's own light/dark/ayu toggle (stored in localStorage).
    // "dark" and "ayu" map to Mermaid's dark theme; everything else uses default.
    let init = r#"document.addEventListener("DOMContentLoaded", function () {
  document.querySelectorAll("pre.language-mermaid").forEach(function (pre) {
    var code = pre.querySelector("code") || pre;
    var div = document.createElement("div");
    div.className = "mermaid";
    div.textContent = code.textContent;
    (pre.closest(".example-wrap") || pre).replaceWith(div);
  });
  var rustdocTheme = localStorage.getItem("rustdoc-theme");
  var isDark = rustdocTheme === "dark" || rustdocTheme === "ayu";
  mermaid.initialize({ startOnLoad: true, theme: isDark ? "dark" : "default" });
});"#;

    format!("\n<script>\n{js}\n</script>\n\n<script>\n{init}\n</script>\n")
}

fn resolve_includes(path: &str) -> String {
    let src =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("could not read {path}: {e}"));
    let base = Path::new(path).parent().unwrap();

    src.lines()
        .map(|line| {
            line.trim()
                .strip_prefix("{{#include ")
                .and_then(|s| s.strip_suffix("}}"))
                .map_or_else(
                    || format!("{line}\n"),
                    |inner| process_include(inner.trim(), base),
                )
        })
        .collect()
}

fn process_include(inner: &str, base: &Path) -> String {
    let (path_str, spec) = match inner.split_once(':') {
        Some((p, s)) => (p.trim(), Some(s.trim())),
        None => (inner, None),
    };

    let path = base.join(path_str);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not include {}: {e}", path.display()));

    match spec {
        None => {
            // pulldown-cmark ends an HTML block at the first blank line, so blank lines
            // inside an inlined SVG would break out of any wrapping <div> block and cause
            // the SVG <text> nodes to render as unstyled inline HTML text instead of SVG.
            if path.extension().is_some_and(|e| e == "svg") {
                return content
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
            }
            content
        }
        Some(s) => apply_spec(&content, s, &path),
    }
}

// Dispatch to one of three extraction strategies based on spec syntax.
fn apply_spec(content: &str, spec: &str, path: &Path) -> String {
    let lines: Vec<&str> = content.lines().collect();

    if is_line_range(spec) {
        apply_line_range(&lines, spec, path)
    } else if spec.starts_with('"') {
        apply_string_markers(&lines, spec, path)
    } else {
        apply_named_anchor(&lines, spec, path)
    }
}

// "N-M", "N-", "-M" — all chars are ASCII digits or a single dash.
fn is_line_range(spec: &str) -> bool {
    spec.contains('-') && spec.chars().all(|c| c.is_ascii_digit() || c == '-')
}

// Include lines N through M inclusive, 1-indexed. Either bound may be omitted.
fn apply_line_range(lines: &[&str], spec: &str, path: &Path) -> String {
    let (start_str, end_str) = spec.split_once('-').unwrap();
    let start = if start_str.is_empty() {
        1
    } else {
        start_str
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("invalid line number in {}", path.display()))
    };
    let end = if end_str.is_empty() {
        lines.len()
    } else {
        end_str
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("invalid line number in {}", path.display()))
    };

    assert!(
        start >= 1 && start <= lines.len(),
        "start line {start} out of range in {}",
        path.display()
    );
    assert!(
        end >= start && end <= lines.len(),
        "end line {end} out of range in {}",
        path.display()
    );

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let one = i + 1;
            if one >= start && one <= end {
                format!("{line}\n")
            } else {
                format!("# {line}\n")
            }
        })
        .collect()
}

// Include from the line containing `start_marker` through the line containing
// `end_marker`, inclusive of both. Spec format: "start":"end".
fn apply_string_markers(lines: &[&str], spec: &str, path: &Path) -> String {
    let (start_marker, end_marker) = parse_string_pair(spec, path);

    let start_idx = lines
        .iter()
        .position(|l| l.contains(start_marker.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "start marker {:?} not found in {}",
                start_marker,
                path.display()
            )
        });

    let end_idx = lines[start_idx + 1..]
        .iter()
        .position(|l| l.contains(end_marker.as_str()))
        .map_or_else(
            || {
                panic!(
                    "end marker {:?} not found after start in {}",
                    end_marker,
                    path.display()
                )
            },
            |i| i + start_idx + 1,
        );

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i >= start_idx && i <= end_idx {
                format!("{line}\n")
            } else {
                format!("# {line}\n")
            }
        })
        .collect()
}

// Include lines between `// ANCHOR: name` and `// ANCHOR_END: name`, exclusive
// of the anchor comment lines themselves.
fn apply_named_anchor(lines: &[&str], anchor: &str, path: &Path) -> String {
    let start_marker = format!("ANCHOR: {anchor}");
    let end_marker = format!("ANCHOR_END: {anchor}");

    let start_idx = lines
        .iter()
        .position(|l| l.contains(start_marker.as_str()))
        .unwrap_or_else(|| panic!("anchor {anchor:?} not found in {}", path.display()));

    let end_idx = lines[start_idx + 1..]
        .iter()
        .position(|l| l.contains(end_marker.as_str()))
        .map_or_else(
            || panic!("anchor end for {anchor:?} not found in {}", path.display()),
            |i| i + start_idx + 1,
        );

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == start_idx || i == end_idx {
                // Omit the ANCHOR marker lines from both visible and hidden output.
                String::new()
            } else if i > start_idx && i < end_idx {
                format!("{line}\n")
            } else {
                format!("# {line}\n")
            }
        })
        .collect()
}

// Parse a pair of quoted strings separated by a colon: "foo":"bar".
fn parse_string_pair(spec: &str, path: &Path) -> (String, String) {
    let mut chars = spec.chars().peekable();
    let start = parse_quoted(&mut chars, path);
    assert_eq!(
        chars.next(),
        Some(':'),
        "expected ':' between string markers in {}",
        path.display()
    );
    let end = parse_quoted(&mut chars, path);
    (start, end)
}

fn parse_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, path: &Path) -> String {
    assert_eq!(
        chars.next(),
        Some('"'),
        "expected '\"' at start of string marker in {}",
        path.display()
    );
    let mut result = String::new();
    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => {
                if let Some(c) = chars.next() {
                    result.push(c);
                }
            }
            Some(c) => result.push(c),
            None => panic!("unterminated string marker in {}", path.display()),
        }
    }
    result
}
