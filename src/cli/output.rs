//! Output formatting for the `serval` CLI.
//!
//! Two formats:
//! - **Table** (default) — human-friendly, two-column key/value layout
//!   with a header line. Goes to stdout.
//! - **JSON** — single object, pretty-printed. Goes to stdout for
//!   consumption by agents / CI / `jq`.
//!
//! The output mode is global to a CLI invocation (chosen by `--json`),
//! so subcommands construct a value and hand it to the appropriate
//! formatter rather than printing inline.

use serde::Serialize;
use serde_json::Value;

/// Which output format the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

/// Render a serializable value to stdout in the chosen format.
///
/// For `Table` we walk the top-level fields of the JSON representation
/// and lay them out as `KEY  value`. Nested objects/arrays are skipped
/// in this minimal formatter — subcommands that need to display nested
/// data should pre-flatten or override.
pub fn print<T: Serialize>(format: OutputFormat, header: Option<&str>, value: &T) {
    match format {
        OutputFormat::Json => print_json(value),
        OutputFormat::Table => print_table(header, value),
    }
}

fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("internal: failed to serialize JSON output: {e}"),
    }
}

fn print_table<T: Serialize>(header: Option<&str>, value: &T) {
    if let Some(h) = header {
        println!("{h}");
        println!();
    }

    let Ok(json) = serde_json::to_value(value) else {
        eprintln!("internal: failed to serialize value for table output");
        return;
    };

    let Value::Object(map) = json else {
        print_json(value);
        return;
    };

    let key_width = map.keys().map(|k| k.len()).max().unwrap_or(0).min(20);

    for (k, v) in &map {
        let display = match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "—".to_string(),
            Value::Array(_) | Value::Object(_) => continue,
        };
        println!(
            "  {:<width$}  {}",
            k.to_uppercase(),
            display,
            width = key_width
        );
    }
}
