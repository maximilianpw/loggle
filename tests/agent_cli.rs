//! Drives the real `loggle` binary the way an agent would: `log`/`pages` with
//! `--json` and `--level` against a page log written straight into a temporary
//! `XDG_STATE_HOME`, so no live viewer session (or pty) is needed.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

const FIXTURE: &str = include_str!("../fixtures/mixed-service-investigation.log");

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

    /// Writes a page log and registers it as owned by this (live) test process.
    fn add_page(&self, id: &str, log: &str) {
        let metadata = serde_json::json!({
            "id": id,
            "pid": std::process::id(),
            "started_unix_seconds": 1,
            "command": "cat fixtures/mixed-service-investigation.log",
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

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}

fn json_lines(output: &Output) -> Vec<Value> {
    assert!(output.status.success(), "stderr: {}", stderr(output));
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn log_json_emits_one_versioned_record_per_event() {
    let state = TestState::new("log-json");
    state.add_page("t", FIXTURE);

    let records = json_lines(&state.run(&["log", "-i", "t", "--json"]));

    assert_eq!(records.len(), 12);
    for record in &records {
        assert_eq!(record["schema_version"], 1);
        assert!(record["sequence"].is_u64());
        assert!(record["source"].is_string());
        assert!(record["level"].is_string());
        assert!(record["message"].is_string());
        assert!(record["properties"].is_object());
        assert!(record["raw"].is_string());
    }
}

#[test]
fn log_json_property_filter_returns_the_whole_multiline_record() {
    let state = TestState::new("log-json-property");
    state.add_page("t", FIXTURE);

    let records = json_lines(&state.run(&[
        "log",
        "-i",
        "t",
        "--property",
        "requestId=fixture-failed",
        "-n",
        "1",
        "--json",
    ]));

    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["source"], "api");
    assert_eq!(record["level"], "error");
    assert_eq!(record["timestamp"], "10:00:00.050");
    assert_eq!(record["message"], "request failed");
    assert_eq!(record["properties"]["statusCode"], 500);
    assert_eq!(record["properties"]["requestId"], "fixture-failed");
    assert_eq!(record["raw"].as_str().unwrap().lines().count(), 7);
}

#[test]
fn log_level_narrows_text_and_json_output() {
    let state = TestState::new("log-level");
    state.add_page("t", FIXTURE);

    let records = json_lines(&state.run(&["log", "-i", "t", "--level", "ERROR", "--json"]));
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record["level"] == "error"));

    let narrowed = json_lines(&state.run(&[
        "log", "-i", "t", "--level", "error", "--source", "worker", "--json",
    ]));
    assert_eq!(narrowed.len(), 1);
    assert_eq!(narrowed[0]["properties"]["errorCode"], 23503);

    let text = state.run(&["log", "-i", "t", "--level", "err", "--source", "worker"]);
    assert!(text.status.success());
    assert_eq!(
        stdout(&text),
        "[worker] ERROR job failed requestId=fixture-failed errorCode=23503\n"
    );
}

#[test]
fn log_json_with_no_matches_prints_nothing_and_succeeds() {
    let state = TestState::new("log-empty");
    state.add_page("t", FIXTURE);

    let output = state.run(&["log", "-i", "t", "--level", "fatal", "--json"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
}

#[test]
fn log_errors_stay_text_on_stderr_with_json() {
    let state = TestState::new("log-missing");

    let output = state.run(&["log", "-i", "missing", "--json"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).starts_with("error: no log page found for id 'missing'"),
        "{}",
        stderr(&output)
    );

    let invalid = state.run(&["log", "-i", "t", "--level", "notice"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(stderr(&invalid).contains("invalid level 'notice'"));
}

#[test]
fn pages_json_lists_versioned_page_objects() {
    let state = TestState::new("pages-json");

    let empty = state.run(&["pages", "--json"]);
    assert!(empty.status.success());
    assert_eq!(stdout(&empty), "");

    state.add_page("t", FIXTURE);
    let pages = json_lines(&state.run(&["pages", "--json"]));

    assert_eq!(
        pages,
        vec![serde_json::json!({
            "schema_version": 1,
            "id": "t",
            "pid": std::process::id(),
            "started_unix_seconds": 1,
            "command": "cat fixtures/mixed-service-investigation.log",
        })]
    );
}
