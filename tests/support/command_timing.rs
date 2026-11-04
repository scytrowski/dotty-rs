//! Opt-in, sanitized timing for external commands in selected integration tests.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::process::{Command, Output};
use std::time::Instant;

pub(crate) const TIMINGS_ENV: &str = "DOTTY_ORACLE_TIMINGS_FILE";

fn category(command: &Command) -> &'static str {
    let program = command.get_program().to_string_lossy();
    let name = std::path::Path::new(program.as_ref())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    match name {
        "scalac" => "scalac",
        "java" => "java",
        "javap" => "javap",
        "git" => "git",
        "sha256sum" => "sha256sum",
        _ => "other",
    }
}

fn safe_operation(operation: &str) -> String {
    operation
        .bytes()
        .take(80)
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                byte as char
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn output(operation: &str, command: &mut Command) -> io::Result<Output> {
    let started = Instant::now();
    let category = category(command);
    let result = command.output();
    record(
        category,
        operation,
        started.elapsed().as_micros(),
        result.as_ref().map_or("failure", |output| {
            if output.status.success() {
                "success"
            } else {
                "failure"
            }
        }),
    );
    result
}

#[allow(dead_code)] // Shared helper API; this target currently captures stdout/stderr.
pub(crate) fn status(
    operation: &str,
    command: &mut Command,
) -> io::Result<std::process::ExitStatus> {
    let started = Instant::now();
    let category = category(command);
    let result = command.status();
    record(
        category,
        operation,
        started.elapsed().as_micros(),
        result.as_ref().map_or("failure", |status| {
            if status.success() {
                "success"
            } else {
                "failure"
            }
        }),
    );
    result
}

fn record(category: &str, operation: &str, elapsed_us: u128, outcome: &str) {
    let Ok(path) = std::env::var(TIMINGS_ENV) else {
        return;
    };
    let operation = safe_operation(operation);
    // Rows contain only normalized category, operation, duration, and outcome.
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = write_event(&mut file, category, &operation, elapsed_us, outcome);
}

fn write_event(
    mut writer: impl Write,
    category: &str,
    operation: &str,
    elapsed_us: u128,
    status: &str,
) -> io::Result<()> {
    writeln!(
        writer,
        "{{\"event\":\"execute\",\"category\":\"{category}\",\"operation\":\"{operation}\",\"elapsed_ms\":{:.3},\"status\":\"{status}\"}}",
        elapsed_us as f64 / 1000.0
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn timing_identifiers_are_safe_for_json_strings() {
        for value in ["scalac", "java", "javap", "sha256sum", "git"] {
            assert!(value.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        }
        assert_eq!(
            super::safe_operation("path/with \"secrets\""),
            "path_with__secrets_"
        );
        assert_eq!(super::safe_operation(&"a".repeat(100)).len(), 80);
    }

    #[test]
    fn event_uses_oracle_jsonl_shape_and_normalizes_labels() {
        let mut row = Vec::new();
        super::write_event(
            &mut row,
            "scalac",
            &super::safe_operation("negative /tmp/source.scala"),
            1234,
            "failure",
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(row).unwrap(),
            "{\"event\":\"execute\",\"category\":\"scalac\",\"operation\":\"negative__tmp_source_scala\",\"elapsed_ms\":1.234,\"status\":\"failure\"}\n"
        );
    }
}
