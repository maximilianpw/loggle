use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use loggle::{LogLevel, LogPageQueryOptions, LogPageRecord, LogPageTailOptions, SourceConfig};

static NEXT_STATE: AtomicU64 = AtomicU64::new(1);

struct TestState {
    root: PathBuf,
    state_dir: PathBuf,
}

impl TestState {
    fn new(name: &str) -> Self {
        let sequence = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "loggle-agent-cli-{}-{sequence}-{name}",
            std::process::id()
        ));
        let state_dir = root.join("loggle");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(state_dir.join("active-pages")).unwrap();
        fs::create_dir_all(state_dir.join("pages")).unwrap();
        Self { root, state_dir }
    }

    fn add_live_page(&self, id: &str, log: &str) {
        self.add_page_with_pid(id, log, std::process::id());
    }

    fn add_page_with_pid(&self, id: &str, log: &str, pid: u32) {
        let metadata = serde_json::json!({
            "id": id,
            "pid": pid,
            "started_unix_seconds": current_unix_seconds(),
            "command": "test command"
        });
        fs::write(
            self.state_dir
                .join("active-pages")
                .join(format!("{id}.json")),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        fs::write(self.state_dir.join("pages").join(format!("{id}.log")), log).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_loggle"))
            .args(args)
            .env("XDG_STATE_HOME", &self.root)
            .output()
            .unwrap()
    }
}

impl Drop for TestState {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}

fn json_lines(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn public_tail_options_retain_the_legacy_struct_literal_shape() {
    let options = LogPageTailOptions {
        line_count: 25,
        source: Some("api".to_string()),
        text: Some("database".to_string()),
        property_filters: vec!["tenantId=tenant-1".to_string()],
        source_config: SourceConfig::with_fields(["service"]),
    };

    assert_eq!(options.line_count, 25);
    assert_eq!(options.source.as_deref(), Some("api"));
    assert_eq!(options.text.as_deref(), Some("database"));
    assert_eq!(
        options.property_filters,
        vec!["tenantId=tenant-1".to_string()]
    );
    assert_eq!(
        options.source_config,
        SourceConfig::with_fields(["service"])
    );
}

#[test]
fn public_query_options_use_constructor_and_field_updates() {
    let mut options = LogPageQueryOptions::new(25);
    options.source = Some("api".to_string());
    options.text = Some("database".to_string());
    options.level = Some(LogLevel::Error);
    options.property_filters = vec!["tenantId=tenant-1".to_string()];
    options.source_config = SourceConfig::with_fields(["service"]);

    assert_eq!(options.line_count, 25);
    assert_eq!(options.source.as_deref(), Some("api"));
    assert_eq!(options.text.as_deref(), Some("database"));
    assert_eq!(options.level, Some(LogLevel::Error));
    assert_eq!(
        options.property_filters,
        vec!["tenantId=tenant-1".to_string()]
    );
    assert_eq!(
        options.source_config,
        SourceConfig::with_fields(["service"])
    );
}

#[test]
fn public_records_keep_external_field_reads_and_serialization() {
    fn assert_serializable<T: serde::Serialize>() {}
    fn read_public_record_fields(record: &LogPageRecord) -> (u32, &str) {
        (record.schema_version, &record.message)
    }

    assert_serializable::<LogPageRecord>();
    let _read_public_record_fields: fn(&LogPageRecord) -> (u32, &str) = read_public_record_fields;
}

#[test]
fn default_pages_output_remains_the_human_table() {
    let state = TestState::new("pages-table");
    state.add_live_page("api", "api | INFO ready\n");

    let output = state.run(&["pages"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let lines = stdout(&output).lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "ID\tPID\tAGE\tCOMMAND");
    let columns = lines[1].split('\t').collect::<Vec<_>>();
    assert_eq!(columns[0], "api");
    assert_eq!(columns[1], std::process::id().to_string());
    assert!(columns[2].ends_with('s'));
    assert_eq!(columns[3], "test command");
}

#[test]
fn pages_jsonl_is_parseable_and_stably_sorted() {
    let state = TestState::new("pages-jsonl");
    state.add_live_page("z-worker", "worker | INFO ready\n");
    state.add_live_page("a-api", "api | INFO ready\n");

    let output = state.run(&["pages", "--format", "jsonl"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let pages = json_lines(&output);
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0]["id"], "a-api");
    assert_eq!(pages[1]["id"], "z-worker");
    assert_eq!(pages[0]["pid"], std::process::id());
    assert_eq!(pages[0]["command"], "test command");
    assert!(pages[0].get("source_fields").is_none());
}

#[test]
fn pages_jsonl_is_empty_when_no_pages_are_active() {
    let state = TestState::new("pages-empty");

    let output = state.run(&["pages", "--format", "jsonl"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn default_unfiltered_log_tail_keeps_physical_line_semantics() {
    let state = TestState::new("raw-physical");
    state.add_live_page("api", "api | ERROR request failed\n{\nstatusCode: 500\n}\n");

    let output = state.run(&["log", "-i", "api", "-n", "2"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "statusCode: 500\n}\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn unfiltered_jsonl_tail_counts_logical_records() {
    let state = TestState::new("jsonl-logical-tail");
    state.add_live_page(
        "api",
        "api | 14:06:58.892 INFO first\n[14:06:58.892] INFO (#1):\n  {\n    statusCode: 200,\n  }\napi | INFO second\n",
    );

    let output = state.run(&["log", "-i", "api", "-n", "1", "--format", "jsonl"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let records = json_lines(&output);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["message"], "second");
    assert_eq!(records[0]["raw"], "api | INFO second");
}

#[test]
fn level_filtered_jsonl_returns_only_matching_logical_records() {
    let state = TestState::new("level-jsonl");
    state.add_live_page(
        "api",
        "api | INFO ready\napi | ERROR first failure\nweb | ERROR second failure\n",
    );

    let output = state.run(&[
        "log", "-i", "api", "-n", "10", "--level", "error", "--format", "jsonl",
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let records = json_lines(&output);
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record["level"] == "error"));
    assert_eq!(records[0]["message"], "first failure");
    assert_eq!(records[1]["message"], "second failure");
}

#[test]
fn level_filtered_raw_uses_the_structured_query_path() {
    let state = TestState::new("level-raw");
    state.add_live_page(
        "api",
        "api | ERROR first failure\napi | INFO ready\napi | ERROR second failure\n",
    );

    let output = state.run(&["log", "-i", "api", "-n", "1", "--level", "error"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "api | ERROR second failure\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn jsonl_preserves_typed_properties_and_multiline_raw_on_one_line() {
    let state = TestState::new("typed-multiline");
    let log = "api | 14:06:58.892 ERROR request failed\n[14:06:58.892] ERROR (#1):\n  {\n    retryable: true,\n    statusCode: 500,\n  }\n";
    state.add_live_page("api", log);

    let output = state.run(&["log", "-i", "api", "--format", "jsonl"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    assert_eq!(stdout(&output).lines().count(), 1);
    let record = &json_lines(&output)[0];
    assert_eq!(record["schema_version"], 1);
    assert_eq!(record["timestamp"], "14:06:58.892");
    assert_eq!(record["properties"]["retryable"], true);
    assert_eq!(record["properties"]["statusCode"], 500);
    assert_eq!(record["raw"], log.trim_end());
    assert!(record.get("sequence").is_none());
}

#[test]
fn combined_agent_filters_match_the_same_event() {
    let state = TestState::new("combined-filters");
    state.add_live_page(
        "api",
        "api | ERROR database failed tenantId=tenant-1 retryable=true\napi | ERROR database failed tenantId=tenant-1 retryable=true skip=true\nweb | ERROR database failed tenantId=tenant-1 retryable=true\napi | INFO database failed tenantId=tenant-1 retryable=true\n",
    );

    let output = state.run(&[
        "log",
        "-i",
        "api",
        "--source",
        "api",
        "--text",
        "database",
        "--level",
        "err",
        "--property",
        "tenantId=tenant-1",
        "--property",
        "retryable=true",
        "--property",
        "!skip",
        "--format",
        "jsonl",
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    let records = json_lines(&output);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["source"], "api");
    assert_eq!(records[0]["level"], "error");
    assert_eq!(records[0]["properties"]["tenantId"], "tenant-1");
}

#[test]
fn no_matching_logs_are_a_silent_success() {
    let state = TestState::new("empty-match");
    state.add_live_page("api", "api | INFO ready\n");

    let output = state.run(&["log", "-i", "api", "--level", "fatal", "--format", "jsonl"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn invalid_level_is_a_usage_error_with_no_stdout() {
    let state = TestState::new("invalid-level");

    let output = state.run(&["log", "-i", "api", "--level", "notice"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("invalid level 'notice'"));
    assert!(stderr(&output).contains("fatal, error, warn, info, debug, trace, unknown"));
}

#[test]
fn missing_page_errors_do_not_contaminate_stdout() {
    let state = TestState::new("missing-page");

    let output = state.run(&["log", "-i", "missing", "--format", "jsonl"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("no log page found for id 'missing'"));
}

#[test]
fn inactive_page_errors_do_not_contaminate_stdout() {
    let state = TestState::new("inactive-page");
    state.add_page_with_pid("inactive", "secret\n", 0);

    let output = state.run(&["log", "-i", "inactive", "--format", "jsonl"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("no log page found for id 'inactive'"));
}
