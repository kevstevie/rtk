use crate::tracking;
use crate::utils::{ok_confirmation, resolved_command, strip_ansi};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::path::Path;

lazy_static! {
    // Build noise patterns
    static ref UP_TO_DATE: Regex = Regex::new(r"^>\s*Task\s+:.*UP-TO-DATE$").unwrap();
    static ref NO_SOURCE: Regex = Regex::new(r"^>\s*Task\s+:.*NO-SOURCE$").unwrap();
    static ref FROM_CACHE: Regex = Regex::new(r"^>\s*Task\s+:.*FROM-CACHE$").unwrap();
    static ref CONFIGURING: Regex = Regex::new(r"^>\s*Configuring project").unwrap();
    static ref RESOLVING: Regex = Regex::new(r"^>\s*Resolving dependencies").unwrap();
    static ref TRANSFORM: Regex = Regex::new(r"^>\s*Transform\s").unwrap();
    static ref DOWNLOAD: Regex = Regex::new(r"^Download(ing)?\s+http").unwrap();
    static ref DAEMON: Regex =
        Regex::new(r"^(Starting a Gradle Daemon|Daemon will be stopped)").unwrap();
    static ref SEPARATOR: Regex = Regex::new(r"^\s*<-+>\s*$").unwrap();
    static ref PROGRESS_DOTS: Regex = Regex::new(r"^\s*\.\s*$").unwrap();
    static ref DEPRECATED_GRADLE: Regex = Regex::new(r"^Deprecated Gradle features").unwrap();
    static ref WARNING_MODE: Regex = Regex::new(r"^You can use '--warning-mode").unwrap();
    static ref SEE_DOCS: Regex = Regex::new(r"^See https://docs\.gradle\.org").unwrap();

    // Test patterns — use `.+` for method names to match parameterized tests with () and []
    static ref TEST_PASSED: Regex = Regex::new(r"^([\w.]+) > (.+) PASSED$").unwrap();
    static ref TEST_FAILED: Regex = Regex::new(r"^([\w.]+) > (.+) FAILED$").unwrap();

    // Dependencies pattern
    static ref DEPENDENCY_DUPLICATE: Regex = Regex::new(r"\(\*\)").unwrap();

    // Tasks patterns
    static ref TASK_WITH_DESC: Regex = Regex::new(r"^(\w+)\s+-\s+(.+)$").unwrap();
    static ref TASK_HEADER: Regex = Regex::new(r"^([\w\s]+tasks|----+)$").unwrap();
}

#[derive(Debug, PartialEq)]
enum TestParseState {
    Scanning,
    InFailure,
}

/// Detect the gradle executable: prefer `./gradlew` if present, else fall back to `gradle`.
fn detect_gradlew_cmd() -> &'static str {
    if Path::new("./gradlew").exists() {
        "./gradlew"
    } else {
        "gradle"
    }
}

/// Main entry point for gradlew commands.
pub fn run(subcommand: String, args: Vec<String>, verbose: u8) -> Result<()> {
    match subcommand.as_str() {
        "build" => run_gradlew_filtered(&subcommand, &args, verbose, filter_gradlew_build),
        "test" => run_gradlew_filtered(&subcommand, &args, verbose, filter_gradlew_test),
        "clean" => run_gradlew_filtered(&subcommand, &args, verbose, filter_gradlew_clean),
        "dependencies" => {
            run_gradlew_filtered(&subcommand, &args, verbose, filter_gradlew_dependencies)
        }
        "tasks" => run_gradlew_filtered(&subcommand, &args, verbose, filter_gradlew_tasks),
        _ => run_passthrough(&subcommand, &args, verbose),
    }
}

/// Run a gradlew subcommand and apply `filter_fn` to its stdout.
fn run_gradlew_filtered<F>(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter_fn: F,
) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();
    let cmd_name = detect_gradlew_cmd();

    let mut cmd = resolved_command(cmd_name);
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {} {}", cmd_name, subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {}. Is Gradle installed?", cmd_name))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    // Strip ANSI escape codes before filtering so regex patterns match correctly.
    let stdout_clean = strip_ansi(&stdout);
    let filtered = filter_fn(&stdout_clean);
    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) =
        crate::tee::tee_and_hint(&raw, &format!("gradlew_{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("{} {} {}", cmd_name, subcommand, args.join(" ")),
        &format!("rtk gradlew {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Passthrough for unsupported subcommands: no filtering, tracking only.
pub fn run_passthrough(subcommand: &str, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let cmd_name = detect_gradlew_cmd();

    let mut cmd = resolved_command(cmd_name);
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running (passthrough): {} {} {}",
            cmd_name,
            subcommand,
            args.join(" ")
        );
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {}. Is Gradle installed?", cmd_name))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    print!("{}", stdout);
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("{} {} {}", cmd_name, subcommand, args.join(" ")),
        &format!(
            "rtk gradlew {} {} (passthrough)",
            subcommand,
            args.join(" ")
        ),
        &raw,
        &raw,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter gradlew build output in a single pass.
/// Successful builds return a compact summary; failed builds show only error sections.
fn filter_gradlew_build(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_error = false;
    let mut actionable_line: Option<&str> = None;
    let mut is_success = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("BUILD SUCCESSFUL") {
            is_success = true;
            continue;
        }
        if trimmed.contains("actionable tasks") {
            actionable_line = Some(trimmed);
            continue;
        }

        // Skip build noise
        if UP_TO_DATE.is_match(line)
            || NO_SOURCE.is_match(line)
            || FROM_CACHE.is_match(line)
            || CONFIGURING.is_match(line)
            || RESOLVING.is_match(line)
            || TRANSFORM.is_match(line)
            || DOWNLOAD.is_match(line)
            || DAEMON.is_match(line)
            || SEPARATOR.is_match(line)
            || PROGRESS_DOTS.is_match(line)
            || DEPRECATED_GRADLE.is_match(line)
            || WARNING_MODE.is_match(line)
            || SEE_DOCS.is_match(line)
        {
            continue;
        }

        // Enter error mode on Gradle error section headers or failed task lines.
        // A failed task line triggers it so subsequent compiler diagnostics are also captured.
        if trimmed.starts_with("FAILURE:")
            || trimmed.starts_with("* What went wrong:")
            || trimmed.starts_with("* Try:")
            || (line.starts_with("> Task :") && line.contains("FAILED"))
        {
            in_error = true;
        }

        if in_error || trimmed.starts_with("BUILD FAILED") || trimmed.starts_with("error:") {
            result.push(line);
        }
    }

    if is_success {
        return ok_confirmation("build", actionable_line.unwrap_or(""));
    }

    if result.is_empty() {
        "BUILD FAILED".to_string()
    } else {
        result.join("\n")
    }
}

/// Filter gradlew test output: show only failed tests with stack traces.
fn filter_gradlew_test(output: &str) -> String {
    let mut state = TestParseState::Scanning;
    let mut result = Vec::new();
    let mut failure_lines = Vec::new();
    let mut failure_count = 0;
    let mut pass_count = 0;
    let mut stack_trace_depth = 0;
    const MAX_STACK_DEPTH: usize = 5;

    for line in output.lines() {
        let trimmed = line.trim();

        if TEST_PASSED.is_match(trimmed) {
            pass_count += 1;
            continue;
        }

        if TEST_FAILED.is_match(trimmed) {
            if !failure_lines.is_empty() {
                result.push(failure_lines.join("\n"));
                failure_lines.clear();
            }
            failure_lines.push(line.to_string());
            state = TestParseState::InFailure;
            failure_count += 1;
            stack_trace_depth = 0;
            continue;
        }

        match state {
            TestParseState::Scanning => {
                if trimmed.starts_with("BUILD SUCCESSFUL")
                    || trimmed.starts_with("BUILD FAILED")
                    || trimmed.contains("tests completed")
                {
                    result.push(line.to_string());
                }
            }
            TestParseState::InFailure => {
                if trimmed.is_empty() {
                    state = TestParseState::Scanning;
                    if !failure_lines.is_empty() {
                        result.push(failure_lines.join("\n"));
                        failure_lines.clear();
                    }
                } else if stack_trace_depth < MAX_STACK_DEPTH {
                    failure_lines.push(format!("  {}", trimmed));
                    stack_trace_depth += 1;
                }
            }
        }
    }

    if !failure_lines.is_empty() {
        result.push(failure_lines.join("\n"));
    }

    if failure_count == 0 {
        ok_confirmation("test", &format!("all {} tests passed", pass_count))
    } else {
        result.join("\n\n")
    }
}

/// Filter gradlew clean: ultra-compact output.
fn filter_gradlew_clean(output: &str) -> String {
    for line in output.lines() {
        if line.trim().starts_with("BUILD FAILED") {
            return line.to_string();
        }
    }
    ok_confirmation("clean", "")
}

/// Filter gradlew dependencies: limit depth and remove duplicates.
fn filter_gradlew_dependencies(output: &str) -> String {
    let mut result = Vec::new();
    let mut line_count = 0;
    const MAX_LINES: usize = 50;
    const MAX_DEPTH: usize = 3;

    for line in output.lines() {
        if line_count >= MAX_LINES {
            result.push("... (truncated, run 'gradle dependencies' for full output)".to_string());
            break;
        }

        let trimmed = line.trim();

        if DEPENDENCY_DUPLICATE.is_match(line)
            || trimmed.starts_with("(*) - Indicates")
            || trimmed.starts_with("A web-based")
            || trimmed.starts_with("BUILD")
            || trimmed.contains("actionable task")
        {
            continue;
        }

        let depth = line
            .bytes()
            .take_while(|b| b.is_ascii_whitespace() || *b == b'|' || *b == b'+' || *b == b'\\')
            .count()
            / 4;

        if depth <= MAX_DEPTH || line.starts_with("---") || line.contains("compileClasspath") {
            result.push(line.to_string());
            line_count += 1;
        }
    }

    if result.is_empty() {
        ok_confirmation("dependencies", "")
    } else {
        result.join("\n")
    }
}

/// Filter gradlew tasks: show only task names, preserve group headers.
fn filter_gradlew_tasks(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with("BUILD") || trimmed.starts_with("To see") {
            continue;
        }

        if TASK_HEADER.is_match(trimmed) {
            result.push(line.to_string());
            continue;
        }

        if let Some(caps) = TASK_WITH_DESC.captures(trimmed) {
            result.push(caps[1].to_string());
        } else if !trimmed.starts_with("---") {
            result.push(line.to_string());
        }
    }

    if result.is_empty() {
        ok_confirmation("tasks", "")
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_gradlew_build_filter() {
        let input = include_str!("../tests/fixtures/gradlew_build_raw.txt");
        let output = filter_gradlew_build(input);

        assert!(!output.contains("UP-TO-DATE"));
        assert!(!output.contains("NO-SOURCE"));
        assert!(!output.contains("Deprecated Gradle features"));
        assert!(output.starts_with("ok build"));
    }

    #[test]
    fn test_gradlew_build_savings() {
        let input = include_str!("../tests/fixtures/gradlew_build_raw.txt");
        let output = filter_gradlew_build(input);

        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings for gradlew build, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_gradlew_test_filter() {
        let input = include_str!("../tests/fixtures/gradlew_test_raw.txt");
        let output = filter_gradlew_test(input);

        assert!(output.contains("testSubtraction FAILED"));
        assert!(output.contains("testReverse FAILED"));
        assert!(!output.contains("testAddition PASSED"));
        assert!(!output.contains("testMultiplication PASSED"));
        assert!(output.contains("AssertionError"));
        assert!(output.contains("NullPointerException"));
    }

    #[test]
    fn test_gradlew_test_savings() {
        let input = include_str!("../tests/fixtures/gradlew_test_raw.txt");
        let output = filter_gradlew_test(input);

        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 80.0,
            "Expected ≥80% savings for gradlew test, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_gradlew_test_all_passed() {
        let input = "> Task :test\n\ncom.example.Test > test1 PASSED\ncom.example.Test > test2 PASSED\n\nBUILD SUCCESSFUL in 1s";
        let output = filter_gradlew_test(input);

        assert!(output.contains("ok test"));
        assert!(output.contains("2 tests passed"));
    }

    #[test]
    fn test_gradlew_clean_filter() {
        let input = "> Task :clean\nBUILD SUCCESSFUL in 1s\n1 actionable task: 1 executed";
        let output = filter_gradlew_clean(input);

        assert_eq!(output, "ok clean");
    }

    #[test]
    fn test_gradlew_dependencies_filter() {
        let input = include_str!("../tests/fixtures/gradlew_dependencies_raw.txt");
        let output = filter_gradlew_dependencies(input);

        assert!(!output.contains("(*)"));
        assert!(output.contains("spring-boot-starter-web") || output.contains("compileClasspath"));
    }

    #[test]
    fn test_gradlew_dependencies_savings() {
        let input = include_str!("../tests/fixtures/gradlew_dependencies_raw.txt");
        let output = filter_gradlew_dependencies(input);

        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 65.0,
            "Expected ≥65% savings for gradlew dependencies, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_gradlew_tasks_filter() {
        let input = include_str!("../tests/fixtures/gradlew_tasks_raw.txt");
        let output = filter_gradlew_tasks(input);

        assert!(
            output.contains("bootRun") || output.contains("assemble") || output.contains("tasks")
        );
        assert!(
            !output.contains("Assembles the outputs of this project")
                || output.lines().count() < 30
        );
    }

    #[test]
    fn test_gradlew_tasks_savings() {
        let input = include_str!("../tests/fixtures/gradlew_tasks_raw.txt");
        let output = filter_gradlew_tasks(input);

        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 70.0,
            "Expected ≥70% savings for gradlew tasks, got {:.1}%",
            savings
        );
    }
}
