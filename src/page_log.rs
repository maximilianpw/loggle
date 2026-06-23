use std::{
    collections::{HashMap, VecDeque},
    env, fmt, fs,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    buffer::LogBuffer,
    filter::{LogFilter, PropertyFilterUpdate},
    model::SourceConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPageId(String);

impl LogPageId {
    pub fn parse(input: &str) -> Result<Self, LogPageIdError> {
        const MAX_LEN: usize = 128;

        let input = input.trim();
        if input.is_empty() {
            return Err(LogPageIdError::new("log page id must not be empty"));
        }

        if input.len() > MAX_LEN {
            return Err(LogPageIdError::new(
                "log page id must be at most 128 characters",
            ));
        }

        if matches!(input, "." | "..") {
            return Err(LogPageIdError::new("log page id must not be . or .."));
        }

        if !input
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(LogPageIdError::new(
                "log page id may only contain letters, numbers, '.', '_' and '-'",
            ));
        }

        Ok(Self(input.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LogPageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogPageId {
    type Err = LogPageIdError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPageIdError {
    message: String,
}

impl LogPageIdError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LogPageIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LogPageIdError {}

#[derive(Debug)]
pub enum LogPageError {
    MissingPage {
        id: LogPageId,
        path: PathBuf,
    },
    ActivePageIdInUse(LogPageId),
    InvalidPropertyFilter(String),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Output(io::Error),
}

impl fmt::Display for LogPageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPage { id, path } => {
                write!(f, "no log page found for id '{id}' at {}", path.display())
            }
            Self::ActivePageIdInUse(id) => {
                write!(f, "log page id '{id}' is already active")
            }
            Self::InvalidPropertyFilter(value) => {
                write!(f, "invalid property filter '{value}'")
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "failed to {action} {}: {source}", path.display()),
            Self::Output(source) => write!(f, "failed to write log output: {source}"),
        }
    }
}

impl std::error::Error for LogPageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingPage { .. }
            | Self::ActivePageIdInUse(_)
            | Self::InvalidPropertyFilter(_) => None,
            Self::Io { source, .. } | Self::Output(source) => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveLogPage {
    pub id: String,
    pub pid: u32,
    pub started_unix_seconds: u64,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPageTailOptions {
    pub line_count: usize,
    pub source: Option<String>,
    pub text: Option<String>,
    pub property_filters: Vec<String>,
    pub source_config: SourceConfig,
}

impl LogPageTailOptions {
    pub fn new(line_count: usize) -> Self {
        Self {
            line_count,
            source: None,
            text: None,
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
        }
    }

    fn has_filters(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| !source.is_empty())
            || self.text.as_ref().is_some_and(|text| !text.is_empty())
            || !self.property_filters.is_empty()
    }
}

pub fn active_log_pages() -> Result<Vec<ActiveLogPage>, LogPageError> {
    active_log_pages_from_dir(&log_page_registry_dir())
}

pub fn print_log_page_tail<W: Write>(
    id: &LogPageId,
    line_count: usize,
    writer: &mut W,
) -> Result<(), LogPageError> {
    print_log_page_tail_with_options(id, &LogPageTailOptions::new(line_count), writer)
}

pub fn print_log_page_tail_with_options<W: Write>(
    id: &LogPageId,
    options: &LogPageTailOptions,
    writer: &mut W,
) -> Result<(), LogPageError> {
    let path = log_page_path(id);
    print_log_page_tail_from_path(id, &path, options, writer)
}

pub(crate) struct PageLogRecorder {
    path: PathBuf,
    writer: BufWriter<File>,
    max_lines: usize,
    lines_written: usize,
}

#[derive(Debug)]
pub(crate) struct ActiveLogPageRegistration {
    path: PathBuf,
}

impl PageLogRecorder {
    pub(crate) fn create(id: &LogPageId, max_lines: usize) -> Result<Self, LogPageError> {
        let path = log_page_path(id);
        Self::create_at_path(path, max_lines)
    }

    fn create_at_path(path: PathBuf, max_lines: usize) -> Result<Self, LogPageError> {
        let dir = path
            .parent()
            .expect("log page paths are created with a parent directory");
        fs::create_dir_all(dir).map_err(|source| LogPageError::Io {
            action: "create log page directory",
            path: dir.to_path_buf(),
            source,
        })?;
        let file = File::create(&path).map_err(|source| LogPageError::Io {
            action: "create log page",
            path: path.clone(),
            source,
        })?;

        Ok(Self {
            path,
            writer: BufWriter::new(file),
            max_lines,
            lines_written: 0,
        })
    }

    pub(crate) fn record_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.writer, "{line}")?;
        self.lines_written += 1;
        // Keep the on-disk log bounded to the same window as the in-memory
        // buffer: let it grow to twice the cap, then compact back down so the
        // rewrite cost is amortised across `max_lines` appends.
        if self.max_lines > 0 && self.lines_written >= self.max_lines.saturating_mul(2) {
            self.compact()?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn compact(&mut self) -> io::Result<()> {
        self.writer.flush()?;
        let retained = {
            let file = File::open(&self.path)?;
            tail_lines(BufReader::new(file), self.max_lines)?
        };
        let mut file = File::create(&self.path)?;
        for line in &retained {
            writeln!(file, "{line}")?;
        }
        file.flush()?;
        self.lines_written = retained.len();
        self.writer = BufWriter::new(file);
        Ok(())
    }
}

impl ActiveLogPageRegistration {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for ActiveLogPageRegistration {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn claim_active_log_page(
    requested: Option<LogPageId>,
    command: impl Into<String>,
) -> Result<(LogPageId, ActiveLogPageRegistration), LogPageError> {
    claim_active_log_page_in_dir(requested, command, &log_page_registry_dir())
}

fn claim_active_log_page_in_dir(
    requested: Option<LogPageId>,
    command: impl Into<String>,
    dir: &Path,
) -> Result<(LogPageId, ActiveLogPageRegistration), LogPageError> {
    let command = command.into();
    // Reaps metadata left behind by dead processes so their ids can be reused.
    let active = active_log_pages_from_dir(dir)?;

    if let Some(requested) = requested {
        let registration = try_register_active_log_page(&requested, &command, dir)?;
        return Ok((requested, registration));
    }

    for candidate in 1u64.. {
        let id = LogPageId(candidate.to_string());
        if active.iter().any(|page| page.id == id.as_str()) {
            continue;
        }
        match try_register_active_log_page(&id, &command, dir) {
            Ok(registration) => return Ok((id, registration)),
            // Lost the race to a concurrently starting session; try the next id.
            Err(LogPageError::ActivePageIdInUse(_)) => continue,
            Err(other) => return Err(other),
        }
    }

    unreachable!("u64 ID space is finite but practically inexhaustible")
}

/// Atomically claims an id by exclusively creating its metadata file, so two
/// sessions starting at once cannot both take the same auto-allocated id.
fn try_register_active_log_page(
    id: &LogPageId,
    command: &str,
    dir: &Path,
) -> Result<ActiveLogPageRegistration, LogPageError> {
    fs::create_dir_all(dir).map_err(|source| LogPageError::Io {
        action: "create active page directory",
        path: dir.to_path_buf(),
        source,
    })?;

    let path = active_log_page_path_in_dir(id, dir);
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            return Err(LogPageError::ActivePageIdInUse(id.clone()));
        }
        Err(source) => {
            return Err(LogPageError::Io {
                action: "create active page metadata",
                path,
                source,
            });
        }
    };

    let entry = ActiveLogPage {
        id: id.as_str().to_string(),
        pid: std::process::id(),
        started_unix_seconds: current_unix_seconds(),
        command: command.to_string(),
    };
    let json = serde_json::to_string_pretty(&entry).expect("active log page metadata serializes");
    file.write_all(json.as_bytes())
        .map_err(|source| LogPageError::Io {
            action: "write active page metadata",
            path: path.clone(),
            source,
        })?;

    Ok(ActiveLogPageRegistration::new(path))
}

fn print_log_page_tail_from_path<W: Write>(
    id: &LogPageId,
    path: &Path,
    options: &LogPageTailOptions,
    writer: &mut W,
) -> Result<(), LogPageError> {
    let file = File::open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            LogPageError::MissingPage {
                id: id.clone(),
                path: path.to_path_buf(),
            }
        } else {
            LogPageError::Io {
                action: "open log page",
                path: path.to_path_buf(),
                source,
            }
        }
    })?;

    let reader = BufReader::new(file);
    let lines = if options.has_filters() {
        filtered_tail_lines(reader, options, path)?
    } else {
        tail_lines(reader, options.line_count).map_err(|source| LogPageError::Io {
            action: "read log page",
            path: path.to_path_buf(),
            source,
        })?
    };

    for line in lines {
        writeln!(writer, "{line}").map_err(LogPageError::Output)?;
    }

    Ok(())
}

fn filtered_tail_lines<R: BufRead>(
    reader: R,
    options: &LogPageTailOptions,
    path: &Path,
) -> Result<Vec<String>, LogPageError> {
    let mut buffer = LogBuffer::unbounded_with_source_config(options.source_config.clone());
    // Track the raw lines that compose each event so a filtered match emits the
    // whole record — header plus any folded multi-line property block — instead
    // of just the header line. A line that does not start a new event is a
    // property-block continuation belonging to the most recently started event.
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut group_of_sequence: HashMap<u64, usize> = HashMap::new();
    let mut current_group: Option<usize> = None;

    for line in reader.lines() {
        let line = line.map_err(|source| LogPageError::Io {
            action: "read log page",
            path: path.to_path_buf(),
            source,
        })?;
        let change = buffer.push_line(line.clone());
        if let Some(sequence) = change.appended {
            let index = groups.len();
            groups.push(vec![line]);
            group_of_sequence.insert(sequence, index);
            current_group = Some(index);
        } else if let Some(index) = current_group {
            groups[index].push(line);
        }
    }

    let filter = log_filter_for_options(options)?;
    let matching = buffer
        .events()
        .iter()
        .filter(|event| filter.matches(event))
        .map(|event| event.sequence)
        .collect::<Vec<_>>();

    let start = matching.len().saturating_sub(options.line_count);
    let mut lines = Vec::new();
    for sequence in &matching[start..] {
        if let Some(&index) = group_of_sequence.get(sequence) {
            lines.extend(groups[index].iter().cloned());
        }
    }

    Ok(lines)
}

fn log_filter_for_options(options: &LogPageTailOptions) -> Result<LogFilter, LogPageError> {
    let mut filter = LogFilter::default();
    filter.source = options
        .source
        .as_ref()
        .map(|source| source.trim().to_string())
        .filter(|source| !source.is_empty());
    filter.text = options
        .text
        .as_ref()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());

    for property_filter in &options.property_filters {
        let update = PropertyFilterUpdate::parse(property_filter, false)
            .ok_or_else(|| LogPageError::InvalidPropertyFilter(property_filter.clone()))?;
        filter.add_property_filter(update);
    }

    Ok(filter)
}

fn tail_lines<R: BufRead>(reader: R, line_count: usize) -> io::Result<Vec<String>> {
    if line_count == 0 {
        return Ok(Vec::new());
    }

    let mut lines = VecDeque::with_capacity(line_count);
    for line in reader.lines() {
        if lines.len() == line_count {
            lines.pop_front();
        }
        lines.push_back(line?);
    }

    Ok(lines.into_iter().collect())
}

fn log_page_path(id: &LogPageId) -> PathBuf {
    log_page_dir().join(format!("{}.log", id.as_str()))
}

fn log_page_dir() -> PathBuf {
    loggle_state_dir().join("pages")
}

fn log_page_registry_dir() -> PathBuf {
    loggle_state_dir().join("active-pages")
}

fn active_log_page_path_in_dir(id: &LogPageId, dir: &Path) -> PathBuf {
    dir.join(format!("{}.json", id.as_str()))
}

fn active_log_pages_from_dir(dir: &Path) -> Result<Vec<ActiveLogPage>, LogPageError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LogPageError::Io {
                action: "read active page directory",
                path: dir.to_path_buf(),
                source,
            });
        }
    };

    let mut pages = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| LogPageError::Io {
            action: "read active page directory entry",
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }

        let input = fs::read_to_string(&path).map_err(|source| LogPageError::Io {
            action: "read active page metadata",
            path: path.clone(),
            source,
        })?;
        let Ok(page) = serde_json::from_str::<ActiveLogPage>(&input) else {
            continue;
        };

        if process_is_active(page.pid) {
            pages.push(page);
        } else {
            // The owning process is gone: drop its metadata and its log file so
            // stale pages do not accumulate in the state directory.
            let _ = fs::remove_file(path);
            if let Ok(id) = LogPageId::parse(&page.id) {
                let _ = fs::remove_file(log_page_path(&id));
            }
        }
    }

    pages.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(pages)
}

fn loggle_state_dir() -> PathBuf {
    if let Some(path) = non_empty_env_path("XDG_STATE_HOME") {
        return path.join("loggle");
    }

    if let Some(home) = non_empty_env_path("HOME") {
        return home.join(".local").join("state").join("loggle");
    }

    env::temp_dir().join("loggle")
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn process_is_active(pid: u32) -> bool {
    if pid == 0 || pid > libc::pid_t::MAX as u32 {
        return false;
    }

    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0
        || io::Error::last_os_error()
            .raw_os_error()
            .is_some_and(|code| code == libc::EPERM)
}

#[cfg(not(unix))]
fn process_is_active(_pid: u32) -> bool {
    // Without a portable liveness probe, assume the page is still active so we
    // neither reap a live page nor reuse its id.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_page_id_accepts_safe_names() {
        assert_eq!(LogPageId::parse("1").unwrap().as_str(), "1");
        assert_eq!(
            LogPageId::parse("agent.api-1").unwrap().as_str(),
            "agent.api-1"
        );
    }

    #[test]
    fn log_page_id_rejects_path_like_names() {
        assert!(LogPageId::parse("").is_err());
        assert!(LogPageId::parse("../api").is_err());
        assert!(LogPageId::parse("api/log").is_err());
    }

    #[test]
    fn tail_lines_keeps_the_last_requested_lines() {
        let input = "one\ntwo\nthree\nfour\n".as_bytes();
        let lines = tail_lines(BufReader::new(input), 2).unwrap();

        assert_eq!(lines, vec!["three".to_string(), "four".to_string()]);
    }

    #[test]
    fn tail_lines_allows_zero_lines() {
        let input = "one\ntwo\n".as_bytes();
        let lines = tail_lines(BufReader::new(input), 0).unwrap();

        assert!(lines.is_empty());
    }

    #[test]
    fn filtered_tail_lines_matches_source() {
        let input = "api | one\nweb | two\napi | three\n".as_bytes();
        let options = LogPageTailOptions {
            line_count: 2,
            source: Some("api".to_string()),
            text: None,
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        assert_eq!(
            lines,
            vec!["api | one".to_string(), "api | three".to_string()]
        );
    }

    #[test]
    fn filtered_tail_lines_matches_property_predicates() {
        let input = "api | INFO request tenantId=tenant-1\napi | INFO request tenantId=tenant-2\nweb | INFO request tenantId=tenant-1\n".as_bytes();
        let options = LogPageTailOptions {
            line_count: 5,
            source: Some("api".to_string()),
            text: None,
            property_filters: vec!["tenantId=tenant-1".to_string()],
            source_config: SourceConfig::default(),
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        assert_eq!(
            lines,
            vec!["api | INFO request tenantId=tenant-1".to_string()]
        );
    }

    #[test]
    fn filtered_tail_lines_matches_properties_from_blocks() {
        let input = "14:06:58.892 INFO request completed\n[14:06:58.892] INFO (#1):\n  {\n    tenantId: \"tenant-1\"\n  }\n".as_bytes();
        let options = LogPageTailOptions {
            line_count: 5,
            source: None,
            text: None,
            property_filters: vec!["tenantId=tenant-1".to_string()],
            source_config: SourceConfig::default(),
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        // The whole record is returned, including the folded property block, so
        // the data that matched the predicate is visible to the reader.
        assert_eq!(
            lines,
            vec![
                "14:06:58.892 INFO request completed".to_string(),
                "[14:06:58.892] INFO (#1):".to_string(),
                "  {".to_string(),
                "    tenantId: \"tenant-1\"".to_string(),
                "  }".to_string(),
            ]
        );
    }

    #[test]
    fn filtered_tail_lines_counts_matching_records_not_lines() {
        let input =
            "api | INFO a tenantId=t1\napi | INFO b tenantId=t1\napi | INFO c tenantId=t1\n"
                .as_bytes();
        let options = LogPageTailOptions {
            line_count: 2,
            source: None,
            text: None,
            property_filters: vec!["tenantId=t1".to_string()],
            source_config: SourceConfig::default(),
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        assert_eq!(
            lines,
            vec![
                "api | INFO b tenantId=t1".to_string(),
                "api | INFO c tenantId=t1".to_string(),
            ]
        );
    }

    #[test]
    fn filtered_tail_lines_matches_text() {
        let input = "api | INFO ready\napi | ERROR database unavailable\nweb | ERROR database unavailable\n"
            .as_bytes();
        let options = LogPageTailOptions {
            line_count: 5,
            source: Some("api".to_string()),
            text: Some("database".to_string()),
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        assert_eq!(lines, vec!["api | ERROR database unavailable".to_string()]);
    }

    #[test]
    fn vev_compose_fixture_replays_buildkit_and_status_filters() {
        let options = LogPageTailOptions {
            line_count: 10,
            source: Some("vev-statistics".to_string()),
            text: Some("npm ci".to_string()),
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
        };

        let lines = filtered_tail_lines(
            BufReader::new(include_str!("../fixtures/vev-compose-smoke.log").as_bytes()),
            &options,
            Path::new("fixtures/vev-compose-smoke.log"),
        )
        .unwrap();

        assert_eq!(
            lines,
            vec![
                "#35 [vev-statistics base 5/7] RUN --mount=type=secret,id=NODE_AUTH_TOKEN sh -c 'npm ci'".to_string(),
                "#35 0.531 npm ci".to_string(),
            ]
        );
    }

    #[test]
    fn page_log_recorder_truncates_and_flushes_lines() {
        let path = env::temp_dir().join(format!(
            "loggle-page-recorder-test-{}.log",
            std::process::id()
        ));
        fs::write(&path, "old\n").unwrap();

        {
            let mut recorder = PageLogRecorder::create_at_path(path.clone(), 100).unwrap();
            recorder.record_line("one").unwrap();
            recorder.record_line("two").unwrap();
            recorder.flush().unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn page_log_recorder_compacts_to_stay_within_bound() {
        let path = env::temp_dir().join(format!(
            "loggle-page-recorder-compact-test-{}.log",
            std::process::id()
        ));

        {
            let mut recorder = PageLogRecorder::create_at_path(path.clone(), 2).unwrap();
            for index in 0..6 {
                recorder.record_line(&format!("line {index}")).unwrap();
            }
            recorder.flush().unwrap();
        }

        let contents = fs::read_to_string(&path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert!(
            lines.len() <= 4,
            "compaction should keep the file bounded, got {lines:?}"
        );
        assert_eq!(lines.last(), Some(&"line 5"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn active_log_page_registration_writes_and_removes_metadata() {
        let dir = env::temp_dir().join(format!("loggle-active-page-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let id = LogPageId::parse("api").unwrap();

        {
            let (claimed, _registration) =
                claim_active_log_page_in_dir(Some(id), "docker compose up", &dir).unwrap();
            assert_eq!(claimed.as_str(), "api");
            let pages = active_log_pages_from_dir(&dir).unwrap();

            assert_eq!(pages.len(), 1);
            assert_eq!(pages[0].id, "api");
            assert_eq!(pages[0].pid, std::process::id());
            assert_eq!(pages[0].command, "docker compose up");
        }

        assert!(active_log_pages_from_dir(&dir).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn active_log_pages_ignore_invalid_metadata_files() {
        let dir = env::temp_dir().join(format!(
            "loggle-invalid-active-page-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("invalid.json"), "{").unwrap();

        assert!(active_log_pages_from_dir(&dir).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn claim_active_log_page_allocates_first_available_numeric_id() {
        let dir = env::temp_dir().join(format!(
            "loggle-allocate-page-id-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let one = LogPageId::parse("1").unwrap();
        let three = LogPageId::parse("3").unwrap();
        let (_, _one) = claim_active_log_page_in_dir(Some(one), "one", &dir).unwrap();
        let (_, _three) = claim_active_log_page_in_dir(Some(three), "three", &dir).unwrap();

        let (id, _registration) = claim_active_log_page_in_dir(None, "two", &dir).unwrap();

        assert_eq!(id.as_str(), "2");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn claim_active_log_page_rejects_active_requested_id() {
        let dir = env::temp_dir().join(format!(
            "loggle-requested-page-id-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let id = LogPageId::parse("api").unwrap();
        let (_, _registration) =
            claim_active_log_page_in_dir(Some(id.clone()), "api", &dir).unwrap();

        let error = claim_active_log_page_in_dir(Some(id), "api", &dir).unwrap_err();

        assert_eq!(error.to_string(), "log page id 'api' is already active");
        let _ = fs::remove_dir_all(dir);
    }
}
