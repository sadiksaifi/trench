use anyhow::Result;
use serde::Serialize;

use crate::output::json::{format_json, format_json_value};
use crate::output::table::Table;
use crate::state::{Database, LogEntry};

/// Extract duration_secs from a LogEntry's JSON payload, if present.
fn extract_duration(entry: &LogEntry) -> Option<f64> {
    extract_duration_from_payload(&entry.payload)
}

/// Extract exit_code from a LogEntry's JSON payload, if present.
fn extract_exit_code(entry: &LogEntry) -> Option<i64> {
    extract_exit_code_from_payload(&entry.payload)
}

/// Format a Unix timestamp as a human-readable datetime string.
fn format_timestamp(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let time_of_day = ts.rem_euclid(86400);
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Compute year/month/day from days since epoch
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Algorithm from Howard Hinnant's civil_from_days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as i64, d as i64)
}

pub fn execute(
    db: &Database,
    repo_id: i64,
    use_color: bool,
    worktree: Option<&str>,
    tail: Option<usize>,
) -> Result<String> {
    let entries = db.list_events_filtered(repo_id, worktree, tail)?;

    if entries.is_empty() {
        return Ok("No events.\n".to_string());
    }

    let mut table = Table::new(vec!["Timestamp", "Type", "Worktree", "Duration", "Exit"]);

    for entry in &entries {
        let ts = format_timestamp(entry.created_at);
        let wt_name = entry.worktree_name.as_deref().unwrap_or("-");
        let duration = match extract_duration(entry) {
            Some(d) => format!("{:.1}s", d),
            None => "-".to_string(),
        };
        let exit = match extract_exit_code(entry) {
            Some(code) => code.to_string(),
            None => "-".to_string(),
        };

        table = table.row(vec![&ts, &entry.event_type, wt_name, &duration, &exit]);
    }

    let rendered = table.render();

    if !use_color {
        return Ok(rendered);
    }

    // Color-code rows: green for success (exit_code 0 or absent), red for failure
    let lines: Vec<&str> = rendered.lines().collect();
    let mut out = String::new();

    // Header line (no color)
    if let Some(header) = lines.first() {
        out.push_str(header);
        out.push('\n');
    }

    for (i, line) in lines.iter().skip(1).enumerate() {
        if i < entries.len() {
            let exit_code = extract_exit_code(&entries[i]);
            match exit_code {
                Some(code) if code != 0 => {
                    out.push_str("\x1b[31m"); // red
                    out.push_str(line);
                    out.push_str("\x1b[0m");
                }
                _ => {
                    out.push_str("\x1b[32m"); // green
                    out.push_str(line);
                    out.push_str("\x1b[0m");
                }
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    Ok(out)
}

#[derive(Serialize)]
struct HookOutputJson {
    event_id: i64,
    event_type: String,
    timestamp: String,
    duration_secs: Option<f64>,
    exit_code: Option<i64>,
    created_at: i64,
    lines: Vec<HookOutputLineJson>,
}

#[derive(Serialize)]
struct HookOutputLineJson {
    stream: String,
    line: String,
    step: Option<String>,
    line_number: i64,
    timestamp: String,
    created_at: i64,
}

/// Extract duration_secs from a JSON payload string.
fn extract_duration_from_payload(payload: &Option<String>) -> Option<f64> {
    payload
        .as_deref()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|v| v.get("duration_secs")?.as_f64())
}

/// Extract exit_code from a JSON payload string.
fn extract_exit_code_from_payload(payload: &Option<String>) -> Option<i64> {
    payload
        .as_deref()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|v| v.get("exit_code")?.as_i64())
}

#[derive(Serialize)]
struct LogEntryJson {
    id: i64,
    timestamp: String,
    event_type: String,
    worktree: Option<String>,
    duration_secs: Option<f64>,
    exit_code: Option<i64>,
    created_at: i64,
}

fn to_json_entry(entry: &LogEntry) -> LogEntryJson {
    LogEntryJson {
        id: entry.id,
        timestamp: format_timestamp(entry.created_at),
        event_type: entry.event_type.clone(),
        worktree: entry.worktree_name.clone(),
        duration_secs: extract_duration(entry),
        exit_code: extract_exit_code(entry),
        created_at: entry.created_at,
    }
}

/// Display stdout/stderr from the last hook execution for a worktree.
///
/// Shows output labeled by step (run/shell) with timestamps.
/// Returns an error if no hook events exist for the worktree.
pub fn execute_output(
    db: &Database,
    repo_id: i64,
    worktree: &str,
) -> Result<String> {
    let event = db
        .get_last_hook_event_for_worktree(repo_id, worktree)?
        .ok_or_else(|| anyhow::anyhow!("No hook output found for worktree '{}'", worktree))?;

    let lines = db.get_hook_output(event.id)?;

    if lines.is_empty() {
        let mut out = String::new();
        out.push_str(&format!("=== {} ({})\n", event.event_type, format_timestamp(event.created_at)));
        out.push_str("(no output captured)\n");
        return Ok(out);
    }

    let mut out = String::new();
    out.push_str(&format!("=== {} ({})\n", event.event_type, format_timestamp(event.created_at)));

    for line in &lines {
        let step_label = line.step.as_deref().unwrap_or("unknown");
        let ts = format_timestamp(line.created_at);
        let stream_marker = if line.stream == "stderr" { "!" } else { " " };
        out.push_str(&format!(
            "[{}]{} {} {}\n",
            step_label, stream_marker, ts, line.line
        ));
    }

    Ok(out)
}

/// JSON output for hook stdout/stderr replay.
pub fn execute_output_json(
    db: &Database,
    repo_id: i64,
    worktree: &str,
) -> Result<String> {
    let event = db
        .get_last_hook_event_for_worktree(repo_id, worktree)?
        .ok_or_else(|| anyhow::anyhow!("No hook output found for worktree '{}'", worktree))?;

    let lines = db.get_hook_output(event.id)?;

    let json_lines: Vec<HookOutputLineJson> = lines
        .iter()
        .map(|l| HookOutputLineJson {
            stream: l.stream.clone(),
            line: l.line.clone(),
            step: l.step.clone(),
            line_number: l.line_number,
            timestamp: format_timestamp(l.created_at),
            created_at: l.created_at,
        })
        .collect();

    let output = HookOutputJson {
        event_id: event.id,
        event_type: event.event_type.clone(),
        timestamp: format_timestamp(event.created_at),
        duration_secs: extract_duration_from_payload(&event.payload),
        exit_code: extract_exit_code_from_payload(&event.payload),
        created_at: event.created_at,
        lines: json_lines,
    };

    format_json_value(&output)
}

/// Internal aggregate stats computed from event entries.
struct SummaryStats {
    total_events: usize,
    hook_runs: usize,
    avg_hook_duration: f64,
    successes: usize,
    failures: usize,
    most_active: Option<(String, usize)>,
}

/// Compute aggregate summary statistics from a list of log entries.
fn compute_summary(entries: &[LogEntry]) -> SummaryStats {
    let hook_entries: Vec<&LogEntry> = entries
        .iter()
        .filter(|e| e.event_type.starts_with("hook:"))
        .collect();

    let durations: Vec<f64> = hook_entries
        .iter()
        .filter_map(|e| extract_duration(e))
        .collect();
    let avg_hook_duration = if durations.is_empty() {
        0.0
    } else {
        durations.iter().sum::<f64>() / durations.len() as f64
    };

    let successes = hook_entries
        .iter()
        .filter(|e| extract_exit_code(e) == Some(0))
        .count();
    let failures = hook_entries
        .iter()
        .filter(|e| matches!(extract_exit_code(e), Some(c) if c != 0))
        .count();

    let most_active = {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for entry in entries {
            if let Some(name) = entry.worktree_name.as_deref() {
                *counts.entry(name).or_insert(0) += 1;
            }
        }
        counts
            .into_iter()
            .max_by(|(name_a, count_a), (name_b, count_b)| {
                count_a.cmp(count_b).then_with(|| name_b.cmp(name_a))
            })
            .map(|(name, count)| (name.to_string(), count))
    };

    SummaryStats {
        total_events: entries.len(),
        hook_runs: hook_entries.len(),
        avg_hook_duration,
        successes,
        failures,
        most_active,
    }
}

/// Display aggregate summary statistics for the event log.
pub fn execute_summary(
    db: &Database,
    repo_id: i64,
) -> Result<String> {
    let entries = db.list_events_filtered(repo_id, None, None)?;

    if entries.is_empty() {
        return Ok("No events recorded yet.\n".to_string());
    }

    let stats = compute_summary(&entries);

    let mut out = String::new();
    out.push_str(&format!("Total events:       {}\n", stats.total_events));
    out.push_str(&format!("Hook runs:          {}\n", stats.hook_runs));
    out.push_str(&format!("Avg hook duration:  {:.1}s\n", stats.avg_hook_duration));
    out.push_str(&format!("Successes:          {}\n", stats.successes));
    out.push_str(&format!("Failures:           {}\n", stats.failures));
    match &stats.most_active {
        Some((name, count)) => {
            out.push_str(&format!("Most active:        {} ({} events)\n", name, count));
        }
        None => {
            out.push_str("Most active:        -\n");
        }
    }

    Ok(out)
}

#[derive(Serialize)]
struct SummaryJson {
    total_events: usize,
    hook_runs: usize,
    avg_hook_duration_secs: f64,
    successes: usize,
    failures: usize,
    most_active_worktree: Option<MostActiveJson>,
}

#[derive(Serialize)]
struct MostActiveJson {
    name: String,
    event_count: usize,
}

/// JSON output for aggregate summary statistics.
pub fn execute_summary_json(
    db: &Database,
    repo_id: i64,
) -> Result<String> {
    let entries = db.list_events_filtered(repo_id, None, None)?;
    let stats = compute_summary(&entries);

    let summary = SummaryJson {
        total_events: stats.total_events,
        hook_runs: stats.hook_runs,
        avg_hook_duration_secs: (stats.avg_hook_duration * 10.0).round() / 10.0,
        successes: stats.successes,
        failures: stats.failures,
        most_active_worktree: stats.most_active.map(|(name, count)| MostActiveJson {
            name,
            event_count: count,
        }),
    };

    format_json_value(&summary)
}

pub fn execute_json(
    db: &Database,
    repo_id: i64,
    worktree: Option<&str>,
    tail: Option<usize>,
) -> Result<String> {
    let entries = db.list_events_filtered(repo_id, worktree, tail)?;
    let json_entries: Vec<LogEntryJson> = entries.iter().map(to_json_entry).collect();
    format_json(&json_entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_summary_empty_state_shows_message() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();

        let output = execute_summary(&db, repo.id).unwrap();
        assert!(output.contains("No events"), "should indicate no events: {output}");
    }

    #[test]
    fn execute_summary_computes_correct_aggregate_stats() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt_a = db
            .insert_worktree(repo.id, "alpha", "feature/alpha", "/wt/a", None)
            .unwrap();
        let wt_b = db
            .insert_worktree(repo.id, "beta", "feature/beta", "/wt/b", None)
            .unwrap();

        // 2 plain events for alpha
        db.insert_event(repo.id, Some(wt_a.id), "created", None).unwrap();
        db.insert_event(repo.id, Some(wt_a.id), "switched", None).unwrap();

        // 3 hook events: 2 success (alpha), 1 failure (beta)
        let ok_payload = serde_json::json!({"exit_code": 0, "duration_secs": 2.0});
        let fail_payload = serde_json::json!({"exit_code": 1, "duration_secs": 4.0});
        db.insert_event(repo.id, Some(wt_a.id), "hook:post_create", Some(&ok_payload)).unwrap();
        db.insert_event(repo.id, Some(wt_a.id), "hook:pre_sync", Some(&ok_payload)).unwrap();
        db.insert_event(repo.id, Some(wt_b.id), "hook:post_create", Some(&fail_payload)).unwrap();

        // 1 plain event for beta
        db.insert_event(repo.id, Some(wt_b.id), "created", None).unwrap();

        let output = execute_summary(&db, repo.id).unwrap();

        // Total events: 6 (2 plain + 3 hooks + 1 plain)
        assert!(output.contains("Total events:       6"), "total events: {output}");
        // Hook runs: 3
        assert!(output.contains("Hook runs:          3"), "hook runs: {output}");
        // Avg duration: (2.0 + 2.0 + 4.0) / 3 = 2.666...
        assert!(output.contains("Avg hook duration:  2.7s"), "avg duration: {output}");
        // Successes: 2, Failures: 1
        assert!(output.contains("Successes:          2"), "successes: {output}");
        assert!(output.contains("Failures:           1"), "failures: {output}");
        // Most active: alpha (4 events: 2 plain + 2 hooks)
        assert!(output.contains("Most active:        alpha (4 events)"), "most active: {output}");
    }

    #[test]
    fn execute_summary_json_empty_state_returns_zeroed_stats() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();

        let output = execute_summary_json(&db, repo.id).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");

        assert_eq!(parsed["total_events"], 0);
        assert_eq!(parsed["hook_runs"], 0);
        assert_eq!(parsed["avg_hook_duration_secs"], 0.0);
        assert_eq!(parsed["successes"], 0);
        assert_eq!(parsed["failures"], 0);
        assert!(parsed["most_active_worktree"].is_null());
    }

    #[test]
    fn execute_summary_json_computes_correct_structured_stats() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt_a = db
            .insert_worktree(repo.id, "alpha", "feature/alpha", "/wt/a", None)
            .unwrap();
        let wt_b = db
            .insert_worktree(repo.id, "beta", "feature/beta", "/wt/b", None)
            .unwrap();

        // 2 plain events for alpha
        db.insert_event(repo.id, Some(wt_a.id), "created", None).unwrap();
        db.insert_event(repo.id, Some(wt_a.id), "switched", None).unwrap();

        // 3 hook events: 2 success, 1 failure
        let ok_payload = serde_json::json!({"exit_code": 0, "duration_secs": 2.0});
        let fail_payload = serde_json::json!({"exit_code": 1, "duration_secs": 4.0});
        db.insert_event(repo.id, Some(wt_a.id), "hook:post_create", Some(&ok_payload)).unwrap();
        db.insert_event(repo.id, Some(wt_a.id), "hook:pre_sync", Some(&ok_payload)).unwrap();
        db.insert_event(repo.id, Some(wt_b.id), "hook:post_create", Some(&fail_payload)).unwrap();

        // 1 plain for beta
        db.insert_event(repo.id, Some(wt_b.id), "created", None).unwrap();

        let output = execute_summary_json(&db, repo.id).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");

        assert_eq!(parsed["total_events"], 6);
        assert_eq!(parsed["hook_runs"], 3);
        assert_eq!(parsed["avg_hook_duration_secs"], 2.7);
        assert_eq!(parsed["successes"], 2);
        assert_eq!(parsed["failures"], 1);

        let most_active = &parsed["most_active_worktree"];
        assert_eq!(most_active["name"], "alpha");
        assert_eq!(most_active["event_count"], 4);
    }

    #[test]
    fn compute_summary_tiebreak_is_deterministic() {
        // When two worktrees have the same event count, the lexicographically
        // smaller name should always win (deterministic tie-breaking).
        let entries = vec![
            LogEntry {
                id: 1,
                event_type: "created".to_string(),
                worktree_name: Some("beta".to_string()),
                payload: None,
                created_at: 1700000000,
            },
            LogEntry {
                id: 2,
                event_type: "created".to_string(),
                worktree_name: Some("alpha".to_string()),
                payload: None,
                created_at: 1700000001,
            },
        ];

        let stats = compute_summary(&entries);
        let (name, count) = stats.most_active.expect("should have most_active");
        assert_eq!(count, 1);
        assert_eq!(name, "alpha", "lexicographically smaller name should win on tie");
    }

    #[test]
    fn execute_output_shows_hook_output_with_step_labels() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feat", "feature/feat", "/wt/feat", None)
            .unwrap();

        // Insert a hook event
        let payload = serde_json::json!({"hook": "post_create", "exit_code": 0, "duration_secs": 1.5});
        let event_id = db
            .insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload))
            .unwrap();

        // Insert log lines with step labels
        db.insert_log(event_id, "stdout", "Installing deps...", 1, Some("run")).unwrap();
        db.insert_log(event_id, "stderr", "warning: peer dep", 2, Some("run")).unwrap();
        db.insert_log(event_id, "stdout", "Migration done", 3, Some("shell")).unwrap();

        let output = execute_output(&db, repo.id, "feat").unwrap();

        // Should contain step labels
        assert!(output.contains("[run]"), "should show [run] step label");
        assert!(output.contains("[shell]"), "should show [shell] step label");

        // Should contain actual output lines
        assert!(output.contains("Installing deps..."), "should show stdout line");
        assert!(output.contains("warning: peer dep"), "should show stderr line");
        assert!(output.contains("Migration done"), "should show shell output");

        // Should contain event type header
        assert!(output.contains("hook:post_create"), "should show event type");
    }

    #[test]
    fn execute_output_json_returns_structured_json() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feat", "feature/feat", "/wt/feat", None)
            .unwrap();

        let payload = serde_json::json!({"hook": "post_create", "exit_code": 0, "duration_secs": 1.5});
        let event_id = db
            .insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload))
            .unwrap();

        db.insert_log(event_id, "stdout", "hello", 1, Some("run")).unwrap();
        db.insert_log(event_id, "stderr", "warn", 2, Some("shell")).unwrap();

        let output = execute_output_json(&db, repo.id, "feat").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");

        // Top-level fields
        assert_eq!(parsed["event_type"], "hook:post_create");
        assert_eq!(parsed["exit_code"], 0);
        assert_eq!(parsed["duration_secs"], 1.5);
        assert!(parsed["timestamp"].is_string());

        // Lines array
        let lines = parsed["lines"].as_array().expect("lines should be array");
        assert_eq!(lines.len(), 2);

        assert_eq!(lines[0]["stream"], "stdout");
        assert_eq!(lines[0]["line"], "hello");
        assert_eq!(lines[0]["step"], "run");
        assert_eq!(lines[0]["line_number"], 1);
        assert!(lines[0]["timestamp"].is_string());

        assert_eq!(lines[1]["stream"], "stderr");
        assert_eq!(lines[1]["step"], "shell");
    }

    #[test]
    fn execute_output_returns_error_when_no_hook_events() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let _wt = db
            .insert_worktree(repo.id, "feat", "feature/feat", "/wt/feat", None)
            .unwrap();

        // Only a non-hook event
        db.insert_event(repo.id, Some(_wt.id), "created", None).unwrap();

        let result = execute_output(&db, repo.id, "feat");
        assert!(result.is_err(), "should error when no hook output exists");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No hook output"), "error message: {err}");
    }

    #[test]
    fn execute_shows_empty_state_message() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();

        let output = execute(&db, repo.id, false, None, None).unwrap();
        assert_eq!(output, "No events.\n");
    }

    #[test]
    fn execute_renders_table_with_event_details() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feature-auth", "feature/auth", "/wt/auth", None)
            .unwrap();

        // Insert a hook event with payload
        let payload = serde_json::json!({"exit_code": 0, "duration_secs": 2.3});
        db.insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload))
            .unwrap();
        // Insert a plain event without payload
        db.insert_event(repo.id, Some(wt.id), "created", None)
            .unwrap();

        let output = execute(&db, repo.id, false, None, None).unwrap();

        // Should have headers
        assert!(output.contains("Timestamp"), "should show Timestamp header");
        assert!(output.contains("Type"), "should show Type header");
        assert!(output.contains("Worktree"), "should show Worktree header");
        assert!(output.contains("Duration"), "should show Duration header");
        assert!(output.contains("Exit"), "should show Exit header");

        // Should show event details
        assert!(
            output.contains("hook:post_create"),
            "should show hook event type"
        );
        assert!(output.contains("created"), "should show created event type");
        assert!(
            output.contains("feature-auth"),
            "should show worktree name"
        );
        assert!(output.contains("2.3s"), "should show duration");
        assert!(output.contains("0"), "should show exit code");
    }

    #[test]
    fn execute_no_color_has_no_ansi() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "wt", "branch", "/wt", None)
            .unwrap();
        db.insert_event(repo.id, Some(wt.id), "created", None)
            .unwrap();

        let output = execute(&db, repo.id, false, None, None).unwrap();
        assert!(
            !output.contains("\x1b"),
            "no-color output must not contain ANSI escapes"
        );
    }

    #[test]
    fn execute_with_color_has_green_for_success() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "wt", "branch", "/wt", None)
            .unwrap();

        let payload = serde_json::json!({"exit_code": 0, "duration_secs": 1.0});
        db.insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload))
            .unwrap();

        let output = execute(&db, repo.id, true, None, None).unwrap();
        assert!(
            output.contains("\x1b[32m"),
            "success events should be green"
        );
    }

    #[test]
    fn execute_with_color_has_red_for_failure() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "wt", "branch", "/wt", None)
            .unwrap();

        let payload = serde_json::json!({"exit_code": 1, "duration_secs": 0.5});
        db.insert_event(repo.id, Some(wt.id), "hook:pre_create", Some(&payload))
            .unwrap();

        let output = execute(&db, repo.id, true, None, None).unwrap();
        assert!(output.contains("\x1b[31m"), "failure events should be red");
    }

    #[test]
    fn execute_json_returns_json_array() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "wt-alpha", "alpha", "/wt/alpha", None)
            .unwrap();

        let payload = serde_json::json!({"exit_code": 0, "duration_secs": 1.5});
        db.insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload))
            .unwrap();
        db.insert_event(repo.id, Some(wt.id), "created", None)
            .unwrap();

        let output = execute_json(&db, repo.id, None, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().expect("should be array");

        assert_eq!(arr.len(), 2, "should have 2 events");

        // Most recent first — "created" was inserted last
        let first = &arr[0];
        assert_eq!(first["event_type"], "created");
        assert_eq!(first["worktree"], "wt-alpha");
        assert!(first["duration_secs"].is_null());
        assert!(first["exit_code"].is_null());

        let second = &arr[1];
        assert_eq!(second["event_type"], "hook:post_create");
        assert_eq!(second["duration_secs"], 1.5);
        assert_eq!(second["exit_code"], 0);
        assert!(second["timestamp"].is_string());
        assert!(second["created_at"].is_number());
    }

    #[test]
    fn execute_json_returns_empty_array_when_no_events() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();

        let output = execute_json(&db, repo.id, None, None).unwrap();
        assert_eq!(output, "[]");
    }

    #[test]
    fn execute_with_tail_limits_output() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "wt", "branch", "/wt", None)
            .unwrap();
        for _ in 0..5 {
            db.insert_event(repo.id, Some(wt.id), "created", None)
                .unwrap();
        }

        let output = execute(&db, repo.id, false, None, Some(2)).unwrap();
        // Header + 2 data rows
        let data_lines: Vec<&str> = output.lines().skip(1).filter(|l| !l.is_empty()).collect();
        assert_eq!(data_lines.len(), 2, "should only show 2 events");
    }

    #[test]
    fn execute_json_with_worktree_filter() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt_a = db
            .insert_worktree(repo.id, "alpha", "feature/alpha", "/wt/a", None)
            .unwrap();
        let wt_b = db
            .insert_worktree(repo.id, "beta", "feature/beta", "/wt/b", None)
            .unwrap();

        db.insert_event(repo.id, Some(wt_a.id), "created", None)
            .unwrap();
        db.insert_event(repo.id, Some(wt_a.id), "switched", None)
            .unwrap();
        db.insert_event(repo.id, Some(wt_b.id), "created", None)
            .unwrap();

        let output = execute_json(&db, repo.id, Some("alpha"), None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2, "should only show alpha's 2 events");
        for entry in arr {
            assert_eq!(entry["worktree"], "alpha");
        }
    }

    #[test]
    fn extract_duration_from_payload() {
        let entry = LogEntry {
            id: 1,
            event_type: "hook:post_create".to_string(),
            worktree_name: Some("wt".to_string()),
            payload: Some(r#"{"duration_secs": 3.14, "exit_code": 0}"#.to_string()),
            created_at: 1700000000,
        };
        assert_eq!(extract_duration(&entry), Some(3.14));
    }

    #[test]
    fn extract_exit_code_from_payload() {
        let entry = LogEntry {
            id: 1,
            event_type: "hook:post_create".to_string(),
            worktree_name: Some("wt".to_string()),
            payload: Some(r#"{"exit_code": 42}"#.to_string()),
            created_at: 1700000000,
        };
        assert_eq!(extract_exit_code(&entry), Some(42));
    }

    #[test]
    fn extract_returns_none_for_missing_payload() {
        let entry = LogEntry {
            id: 1,
            event_type: "created".to_string(),
            worktree_name: None,
            payload: None,
            created_at: 1700000000,
        };
        assert_eq!(extract_duration(&entry), None);
        assert_eq!(extract_exit_code(&entry), None);
    }

    #[test]
    fn format_timestamp_produces_valid_datetime() {
        // 2023-11-14 22:13:20 UTC
        let ts = 1700000000;
        let result = format_timestamp(ts);
        assert_eq!(result, "2023-11-14 22:13:20");
    }
}
