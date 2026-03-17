use anyhow::Result;
use serde::Serialize;

use crate::output::json::format_json;
use crate::output::table::Table;
use crate::state::{Database, LogEntry};

/// Extract duration_secs from a LogEntry's JSON payload, if present.
fn extract_duration(entry: &LogEntry) -> Option<f64> {
    entry
        .payload
        .as_deref()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|v| v.get("duration_secs")?.as_f64())
}

/// Extract exit_code from a LogEntry's JSON payload, if present.
fn extract_exit_code(entry: &LogEntry) -> Option<i64> {
    entry
        .payload
        .as_deref()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|v| v.get("exit_code")?.as_i64())
}

/// Format a Unix timestamp as a human-readable datetime string.
fn format_timestamp(ts: i64) -> String {
    let secs = ts;
    let days = secs / 86400;
    let time_of_day = secs % 86400;
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

pub fn execute(db: &Database, repo_id: i64, use_color: bool) -> Result<String> {
    let entries = db.list_all_events(repo_id)?;

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

pub fn execute_json(db: &Database, repo_id: i64) -> Result<String> {
    let entries = db.list_all_events(repo_id)?;
    let json_entries: Vec<LogEntryJson> = entries.iter().map(to_json_entry).collect();
    format_json(&json_entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_shows_empty_state_message() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();

        let output = execute(&db, repo.id, false).unwrap();
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

        let output = execute(&db, repo.id, false).unwrap();

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

        let output = execute(&db, repo.id, false).unwrap();
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

        let output = execute(&db, repo.id, true).unwrap();
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

        let output = execute(&db, repo.id, true).unwrap();
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

        let output = execute_json(&db, repo.id).unwrap();
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

        let output = execute_json(&db, repo.id).unwrap();
        assert_eq!(output, "[]");
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
