use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    env, fmt, fs,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::fd::AsRawFd;

use crate::{
    buffer::LogBuffer,
    facet::{
        FacetGroup, FacetOptions, MAX_FACET_RECORD_LIMIT, MIN_FACET_RECORD_LIMIT, aggregate_facets,
    },
    filter::{LogFilter, PropertyFilterUpdate},
    model::{Level, LogEvent, LogProperty, PropertyValue, SourceConfig},
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

#[non_exhaustive]
#[derive(Debug)]
pub enum FacetQueryError {
    InvalidRecordWindow { value: usize },
    Page(LogPageError),
}

impl fmt::Display for FacetQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRecordWindow { value } => write!(
                f,
                "facet record window {value} is outside {}..={}",
                MIN_FACET_RECORD_LIMIT, MAX_FACET_RECORD_LIMIT
            ),
            Self::Page(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for FacetQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRecordWindow { .. } => None,
            Self::Page(error) => Some(error),
        }
    }
}

impl From<LogPageError> for FacetQueryError {
    fn from(error: LogPageError) -> Self {
        Self::Page(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveLogPage {
    pub id: String,
    pub pid: u32,
    pub started_unix_seconds: u64,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveLogPageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_version: Option<u32>,
    id: String,
    pid: u32,
    started_unix_seconds: u64,
    command: String,
    #[serde(default)]
    source_fields: Vec<String>,
    #[serde(default)]
    log_file: Option<String>,
    #[serde(default = "metadata_ready_default")]
    ready: bool,
}

const ACTIVE_LOG_PAGE_LEASE_VERSION: u32 = 1;

impl ActiveLogPageMetadata {
    fn public_page(&self) -> ActiveLogPage {
        ActiveLogPage {
            id: self.id.clone(),
            pid: self.pid,
            started_unix_seconds: self.started_unix_seconds,
            command: self.command.clone(),
        }
    }

    fn uses_stable_lock(&self) -> bool {
        cfg!(unix) && self.lease_version == Some(ACTIVE_LOG_PAGE_LEASE_VERSION)
    }
}

fn metadata_ready_default() -> bool {
    true
}

struct ResolvedLogPage {
    metadata: ActiveLogPageMetadata,
    log_path: PathBuf,
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
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPageQueryOptions {
    pub line_count: usize,
    pub source: Option<String>,
    pub text: Option<String>,
    pub level: Option<Level>,
    pub property_filters: Vec<String>,
    pub source_config: SourceConfig,
}

impl LogPageQueryOptions {
    pub fn new(line_count: usize) -> Self {
        Self {
            line_count,
            source: None,
            text: None,
            level: None,
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
        }
    }
}

impl From<&LogPageTailOptions> for LogPageQueryOptions {
    fn from(options: &LogPageTailOptions) -> Self {
        Self {
            line_count: options.line_count,
            source: options.source.clone(),
            text: options.text.clone(),
            level: None,
            property_filters: options.property_filters.clone(),
            source_config: options.source_config.clone(),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogPageRecord {
    pub schema_version: u32,
    pub source: String,
    pub timestamp: Option<String>,
    pub level: Level,
    pub message: String,
    pub properties: BTreeMap<String, serde_json::Value>,
    pub raw: String,
}

impl LogPageRecord {
    const SCHEMA_VERSION: u32 = 1;

    fn from_parsed(record: &ParsedLogPageRecord) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            source: record.event.source.clone(),
            timestamp: record.event.timestamp.clone(),
            level: record.event.level,
            message: record.event.message.clone(),
            properties: record_properties(&record.event.properties),
            raw: record.raw.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedLogPageRecord {
    pub(crate) event: LogEvent,
    pub(crate) raw: String,
}

struct ResolvedLogPageQuery {
    log_path: PathBuf,
    options: LogPageQueryOptions,
    filter: LogFilter,
}

pub fn active_log_pages() -> Result<Vec<ActiveLogPage>, LogPageError> {
    active_log_pages_from_dirs(&log_page_registry_dir(), &log_page_dir())
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
    print_log_page_tail_from_dirs(
        id,
        options,
        writer,
        &log_page_registry_dir(),
        &log_page_dir(),
    )
}

pub fn print_log_page_query<W: Write>(
    id: &LogPageId,
    options: &LogPageQueryOptions,
    writer: &mut W,
) -> Result<(), LogPageError> {
    print_log_page_query_from_dirs(
        id,
        options,
        writer,
        &log_page_registry_dir(),
        &log_page_dir(),
    )
}

pub fn query_log_page_records(
    id: &LogPageId,
    options: &LogPageQueryOptions,
) -> Result<Vec<LogPageRecord>, LogPageError> {
    query_log_page_records_from_dirs(id, options, &log_page_registry_dir(), &log_page_dir())
}

pub fn query_log_page_facets(
    id: &LogPageId,
    options: &LogPageQueryOptions,
    facet_options: &FacetOptions,
) -> Result<Vec<FacetGroup>, FacetQueryError> {
    query_log_page_facets_from_dirs(
        id,
        options,
        facet_options,
        &log_page_registry_dir(),
        &log_page_dir(),
    )
}

fn print_log_page_tail_from_dirs<W: Write>(
    id: &LogPageId,
    options: &LogPageTailOptions,
    writer: &mut W,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<(), LogPageError> {
    print_log_page_query_from_dirs(
        id,
        &LogPageQueryOptions::from(options),
        writer,
        registry_dir,
        page_dir,
    )
}

fn print_log_page_query_from_dirs<W: Write>(
    id: &LogPageId,
    options: &LogPageQueryOptions,
    writer: &mut W,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<(), LogPageError> {
    let query = resolve_log_page_query(id, options, registry_dir, page_dir)?;
    print_resolved_log_page_tail(id, &query, writer)
}

fn query_log_page_records_from_dirs(
    id: &LogPageId,
    options: &LogPageQueryOptions,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<Vec<LogPageRecord>, LogPageError> {
    let query = resolve_log_page_query(id, options, registry_dir, page_dir)?;
    let records =
        parsed_log_page_records_from_path(id, &query.log_path, &query.options.source_config)?;

    Ok(
        tail_matching_records(&records, &query.filter, query.options.line_count)
            .into_iter()
            .map(LogPageRecord::from_parsed)
            .collect(),
    )
}

fn query_log_page_facets_from_dirs(
    id: &LogPageId,
    options: &LogPageQueryOptions,
    facet_options: &FacetOptions,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<Vec<FacetGroup>, FacetQueryError> {
    if !(MIN_FACET_RECORD_LIMIT..=MAX_FACET_RECORD_LIMIT).contains(&options.line_count) {
        return Err(FacetQueryError::InvalidRecordWindow {
            value: options.line_count,
        });
    }

    let query = resolve_log_page_query(id, options, registry_dir, page_dir)?;
    let records =
        parsed_log_page_records_from_path(id, &query.log_path, &query.options.source_config)?;
    Ok(aggregate_facets(
        records.iter().map(|record| &record.event),
        query.options.line_count,
        &query.filter,
        facet_options,
    ))
}

fn resolve_log_page_query(
    id: &LogPageId,
    options: &LogPageQueryOptions,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<ResolvedLogPageQuery, LogPageError> {
    let page = resolve_active_log_page(id, registry_dir, page_dir)?;
    let mut effective_options = options.clone();
    effective_options.source_config = options
        .source_config
        .merged_with_configured_fields(&page.metadata.source_fields);
    let filter = log_filter_for_options(&effective_options)?;

    Ok(ResolvedLogPageQuery {
        log_path: page.log_path,
        options: effective_options,
        filter,
    })
}

struct PageLogRecorder {
    path: PathBuf,
    writer: BufWriter<File>,
    max_lines: usize,
    lines_written: usize,
}

#[derive(Debug)]
struct ActiveLogPageRegistration {
    path: PathBuf,
    metadata: ActiveLogPageMetadata,
    lock: Option<PageIdLock>,
}

#[derive(Debug)]
struct PageIdLock {
    #[cfg(unix)]
    file: File,
}

pub(crate) struct PageLogSession {
    id: LogPageId,
    recorder: Option<PageLogRecorder>,
    registration: Option<ActiveLogPageRegistration>,
    log_path: PathBuf,
}

impl PageLogSession {
    pub(crate) fn start(
        requested: Option<LogPageId>,
        command: impl Into<String>,
        source_config: &SourceConfig,
        max_lines: usize,
    ) -> Result<Self, LogPageError> {
        Self::start_in_dirs_inner(
            requested,
            command,
            source_config,
            max_lines,
            &log_page_registry_dir(),
            &log_page_dir(),
        )
    }

    #[cfg(test)]
    fn start_in_dirs(
        requested: Option<LogPageId>,
        command: impl Into<String>,
        source_config: &SourceConfig,
        max_lines: usize,
        registry_dir: &Path,
        page_dir: &Path,
    ) -> Result<Self, LogPageError> {
        Self::start_in_dirs_inner(
            requested,
            command,
            source_config,
            max_lines,
            registry_dir,
            page_dir,
        )
    }

    fn start_in_dirs_inner(
        requested: Option<LogPageId>,
        command: impl Into<String>,
        source_config: &SourceConfig,
        max_lines: usize,
        registry_dir: &Path,
        page_dir: &Path,
    ) -> Result<Self, LogPageError> {
        let (id, registration) = claim_active_log_page_in_dirs(
            requested,
            command,
            source_config,
            registry_dir,
            page_dir,
        )?;
        Self::from_claim(id, registration, page_dir, max_lines)
    }

    fn from_claim(
        id: LogPageId,
        mut registration: ActiveLogPageRegistration,
        page_dir: &Path,
        max_lines: usize,
    ) -> Result<Self, LogPageError> {
        let log_file = registration
            .metadata
            .log_file
            .as_deref()
            .expect("pending page metadata binds a log generation");
        let recorder = match PageLogRecorder::create_bound(&id, log_file, page_dir, max_lines) {
            Ok(recorder) => recorder,
            Err(error) => {
                // The exact path may be a pre-existing collision. It was never
                // opened by this session, so release only metadata and lock.
                drop(registration);
                return Err(error);
            }
        };
        let log_path = recorder.path.clone();
        if let Err(error) = registration.publish() {
            drop(recorder);
            // Keep the ID lease until its exact generation data is gone.
            remove_file_best_effort(&log_path);
            drop(registration);
            return Err(error);
        }

        Ok(Self {
            id,
            recorder: Some(recorder),
            registration: Some(registration),
            log_path,
        })
    }

    pub(crate) fn id(&self) -> &LogPageId {
        &self.id
    }

    pub(crate) fn record_line(&mut self, line: &str) -> io::Result<()> {
        self.recorder
            .as_mut()
            .expect("live page log session retains its recorder")
            .record_line(line)
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.recorder
            .as_mut()
            .expect("live page log session retains its recorder")
            .flush()
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(
        requested: Option<LogPageId>,
        command: impl Into<String>,
        source_config: &SourceConfig,
        max_lines: usize,
        registry_dir: &Path,
        page_dir: &Path,
    ) -> Result<Self, LogPageError> {
        Self::start_in_dirs(
            requested,
            command,
            source_config,
            max_lines,
            registry_dir,
            page_dir,
        )
    }
}

impl Drop for PageLogSession {
    fn drop(&mut self) {
        if let Some(mut recorder) = self.recorder.take() {
            let _ = recorder.flush();
            drop(recorder);
        }

        // Registration is the lease that prevents this ID from being reused.
        // Remove data while that lease is held, then release registration last
        // so an older session can never unlink its successor's log file.
        remove_file_best_effort(&self.log_path);
        if let Some(registration) = self.registration.take() {
            drop(registration);
        }
    }
}

impl PageLogRecorder {
    fn create_bound(
        id: &LogPageId,
        file_name: &str,
        dir: &Path,
        max_lines: usize,
    ) -> Result<Self, LogPageError> {
        fs::create_dir_all(dir).map_err(|source| LogPageError::Io {
            action: "create log page directory",
            path: dir.to_path_buf(),
            source,
        })?;

        let path = generation_log_path_in_dir(id, file_name, dir)
            .expect("pending page metadata binds a valid generation name");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| LogPageError::Io {
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

    #[cfg(test)]
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

    fn record_line(&mut self, line: &str) -> io::Result<()> {
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

    fn flush(&mut self) -> io::Result<()> {
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
    fn new(path: PathBuf, metadata: ActiveLogPageMetadata, lock: PageIdLock) -> Self {
        Self {
            path,
            metadata,
            lock: Some(lock),
        }
    }

    fn publish(&mut self) -> Result<(), LogPageError> {
        self.metadata.ready = true;
        self.replace_metadata_atomically("publish active page metadata")
    }

    fn replace_metadata_atomically(&self, action: &'static str) -> Result<(), LogPageError> {
        let json =
            serde_json::to_string_pretty(&self.metadata).expect("active page metadata serializes");
        let dir = self
            .path
            .parent()
            .expect("active page metadata paths have a parent directory");

        loop {
            let temp_path = next_metadata_publish_path(&self.path, dir);
            let mut file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
            {
                Ok(file) => file,
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(LogPageError::Io {
                        action,
                        path: self.path.clone(),
                        source,
                    });
                }
            };
            if let Err(source) = file.write_all(json.as_bytes()).and_then(|()| file.flush()) {
                drop(file);
                remove_file_best_effort(&temp_path);
                return Err(LogPageError::Io {
                    action,
                    path: self.path.clone(),
                    source,
                });
            }
            drop(file);

            // Renaming over the still-present lease path publishes one complete
            // metadata generation without ever making the ID claim disappear.
            if let Err(source) = fs::rename(&temp_path, &self.path) {
                remove_file_best_effort(&temp_path);
                return Err(LogPageError::Io {
                    action,
                    path: self.path.clone(),
                    source,
                });
            }
            return Ok(());
        }
    }
}

impl Drop for ActiveLogPageRegistration {
    fn drop(&mut self) {
        remove_file_best_effort(&self.path);
        drop(self.lock.take());
    }
}

impl PageIdLock {
    fn try_acquire(id: &LogPageId, dir: &Path) -> Result<Option<Self>, LogPageError> {
        let path = active_log_page_lock_path_in_dir(id, dir);

        #[cfg(unix)]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|source| LogPageError::Io {
                    action: "open active page lock",
                    path: path.clone(),
                    source,
                })?;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Some(Self { file }));
            }

            let source = io::Error::last_os_error();
            let is_busy = source
                .raw_os_error()
                .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN);
            if is_busy {
                Ok(None)
            } else {
                Err(LogPageError::Io {
                    action: "lock active page",
                    path,
                    source,
                })
            }
        }

        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(Some(Self {}))
        }
    }
}

#[cfg(unix)]
impl Drop for PageIdLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn remove_file_best_effort(path: &Path) {
    let _ = fs::remove_file(path);
}

fn remove_file_if_exists(path: &Path, action: &'static str) -> Result<(), LogPageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(LogPageError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn claim_active_log_page_in_dirs(
    requested: Option<LogPageId>,
    command: impl Into<String>,
    source_config: &SourceConfig,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<(LogPageId, ActiveLogPageRegistration), LogPageError> {
    let command = command.into();
    if let Some(requested) = requested {
        let registration = try_register_active_log_page(
            &requested,
            &command,
            source_config,
            registry_dir,
            page_dir,
        )?;
        return Ok((requested, registration));
    }

    // Auto-allocation needs the global view. Explicit IDs are claimed directly
    // above so an unrelated broken entry cannot prevent a targeted claim.
    let active = active_log_pages_from_dirs(registry_dir, page_dir)?;

    for candidate in 1u64.. {
        let id = LogPageId(candidate.to_string());
        if active.iter().any(|page| page.id == id.as_str()) {
            continue;
        }
        match try_register_active_log_page(&id, &command, source_config, registry_dir, page_dir) {
            Ok(registration) => return Ok((id, registration)),
            // Lost the race to a concurrently starting session; try the next id.
            Err(LogPageError::ActivePageIdInUse(_)) => continue,
            Err(other) => return Err(other),
        }
    }

    unreachable!("u64 ID space is finite but practically inexhaustible")
}

/// Claims the stable per-ID lock before replacing stale metadata and creating
/// a pending metadata lease. The metadata `create_new` keeps this compatible
/// with legacy sessions that do not know about the advisory lock.
fn try_register_active_log_page(
    id: &LogPageId,
    command: &str,
    source_config: &SourceConfig,
    dir: &Path,
    page_dir: &Path,
) -> Result<ActiveLogPageRegistration, LogPageError> {
    fs::create_dir_all(dir).map_err(|source| LogPageError::Io {
        action: "create active page directory",
        path: dir.to_path_buf(),
        source,
    })?;

    let Some(lock) = PageIdLock::try_acquire(id, dir)? else {
        return Err(LogPageError::ActivePageIdInUse(id.clone()));
    };

    let path = active_log_page_path_in_dir(id, dir);
    prepare_active_log_page_claim(id, &path, page_dir)?;
    let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
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

    let metadata = ActiveLogPageMetadata {
        lease_version: Some(ACTIVE_LOG_PAGE_LEASE_VERSION),
        id: id.as_str().to_string(),
        pid: std::process::id(),
        started_unix_seconds: current_unix_seconds(),
        command: command.to_string(),
        source_fields: source_config.configured_fields().to_vec(),
        log_file: Some(next_generation_log_file_name(id)),
        ready: false,
    };
    let registration = ActiveLogPageRegistration::new(path.clone(), metadata, lock);
    let json = serde_json::to_string_pretty(&registration.metadata)
        .expect("pending active page metadata serializes");
    let mut file = file;
    if let Err(source) = file.write_all(json.as_bytes()).and_then(|()| file.flush()) {
        drop(file);
        drop(registration);
        return Err(LogPageError::Io {
            action: "write pending active page metadata",
            path,
            source,
        });
    }
    drop(file);

    Ok(registration)
}

fn prepare_active_log_page_claim(
    id: &LogPageId,
    metadata_path: &Path,
    page_dir: &Path,
) -> Result<(), LogPageError> {
    let input = match fs::read(metadata_path) {
        Ok(input) => input,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(LogPageError::Io {
                action: "read existing active page metadata",
                path: metadata_path.to_path_buf(),
                source,
            });
        }
    };

    let Ok(metadata) = serde_json::from_slice::<ActiveLogPageMetadata>(&input) else {
        return remove_file_if_exists(metadata_path, "remove invalid active page metadata");
    };

    if metadata.id != id.as_str() {
        return remove_file_if_exists(metadata_path, "remove mismatched active page metadata");
    }

    // Acquiring the stable lock proves a v1 owner is gone, even when its PID
    // has already been reused. Legacy owners never held this lock, so retain
    // their PID-based liveness check during the compatibility window.
    if !metadata.uses_stable_lock() && process_is_active(metadata.pid) {
        return Err(LogPageError::ActivePageIdInUse(id.clone()));
    }

    reap_stale_log_page(&metadata, metadata_path, page_dir)
}

fn print_resolved_log_page_tail<W: Write>(
    id: &LogPageId,
    query: &ResolvedLogPageQuery,
    writer: &mut W,
) -> Result<(), LogPageError> {
    if !query.filter.has_active_filters() {
        let file = open_log_page(id, &query.log_path)?;
        let lines =
            tail_lines(BufReader::new(file), query.options.line_count).map_err(|source| {
                LogPageError::Io {
                    action: "read log page",
                    path: query.log_path.clone(),
                    source,
                }
            })?;

        for line in lines {
            writeln!(writer, "{line}").map_err(LogPageError::Output)?;
        }
        return Ok(());
    }

    let records =
        parsed_log_page_records_from_path(id, &query.log_path, &query.options.source_config)?;

    for record in tail_matching_records(&records, &query.filter, query.options.line_count) {
        writeln!(writer, "{}", record.raw).map_err(LogPageError::Output)?;
    }

    Ok(())
}

fn open_log_page(id: &LogPageId, path: &Path) -> Result<File, LogPageError> {
    File::open(path).map_err(|source| {
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
    })
}

fn parsed_log_page_records_from_path(
    id: &LogPageId,
    path: &Path,
    source_config: &SourceConfig,
) -> Result<Vec<ParsedLogPageRecord>, LogPageError> {
    let file = open_log_page(id, path)?;
    parsed_log_page_records(BufReader::new(file), source_config, path)
}

fn parsed_log_page_records<R: BufRead>(
    reader: R,
    source_config: &SourceConfig,
    path: &Path,
) -> Result<Vec<ParsedLogPageRecord>, LogPageError> {
    let mut buffer = LogBuffer::unbounded_with_source_config(source_config.clone());
    // Keep the complete raw group beside its parsed event. Property blocks can
    // update a preceding event or replace a deferred header event, so sequence
    // IDs are the stable association while replay is in progress.
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
        let removed_group_lines =
            take_removed_group_lines(&groups, &mut group_of_sequence, &change.removed);
        if let Some(sequence) = change.appended {
            let index = groups.len();
            let mut group = removed_group_lines;
            group.push(line);
            groups.push(group);
            group_of_sequence.insert(sequence, index);
            current_group = Some(index);
        } else if let Some(index) = current_group {
            groups[index].push(line);
        }
    }

    Ok(buffer
        .events()
        .iter()
        .filter_map(|event| {
            let &index = group_of_sequence.get(&event.sequence)?;
            Some(ParsedLogPageRecord {
                event: event.clone(),
                raw: groups[index].join("\n"),
            })
        })
        .collect())
}

fn tail_matching_records<'a>(
    records: &'a [ParsedLogPageRecord],
    filter: &LogFilter,
    line_count: usize,
) -> Vec<&'a ParsedLogPageRecord> {
    let matching = records
        .iter()
        .filter(|record| filter.matches(&record.event))
        .collect::<Vec<_>>();
    let start = matching.len().saturating_sub(line_count);
    matching.into_iter().skip(start).collect()
}

#[cfg(test)]
fn filtered_tail_lines<R: BufRead>(
    reader: R,
    options: &LogPageTailOptions,
    path: &Path,
) -> Result<Vec<String>, LogPageError> {
    filtered_query_tail_lines(reader, &LogPageQueryOptions::from(options), path)
}

#[cfg(test)]
fn filtered_query_tail_lines<R: BufRead>(
    reader: R,
    options: &LogPageQueryOptions,
    path: &Path,
) -> Result<Vec<String>, LogPageError> {
    let filter = log_filter_for_options(options)?;
    let records = parsed_log_page_records(reader, &options.source_config, path)?;
    Ok(tail_matching_records(&records, &filter, options.line_count)
        .into_iter()
        .flat_map(|record| record.raw.split('\n').map(str::to_string))
        .collect())
}

fn take_removed_group_lines(
    groups: &[Vec<String>],
    group_of_sequence: &mut HashMap<u64, usize>,
    removed: &[u64],
) -> Vec<String> {
    let mut indexes = Vec::new();
    for sequence in removed {
        let Some(index) = group_of_sequence.remove(sequence) else {
            continue;
        };
        if !indexes.contains(&index) {
            indexes.push(index);
        }
    }

    indexes
        .into_iter()
        .flat_map(|index| groups[index].iter().cloned())
        .collect()
}

fn log_filter_for_options(options: &LogPageQueryOptions) -> Result<LogFilter, LogPageError> {
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
    filter.level = options.level;

    for property_filter in &options.property_filters {
        let update = PropertyFilterUpdate::parse(property_filter, false)
            .ok_or_else(|| LogPageError::InvalidPropertyFilter(property_filter.clone()))?;
        filter.add_property_filter(update);
    }

    Ok(filter)
}

fn record_properties(properties: &[LogProperty]) -> BTreeMap<String, serde_json::Value> {
    let mut values = BTreeMap::new();
    for property in properties {
        values
            .entry(property.key.clone())
            .or_insert_with(|| property_json_value(&property.value));
    }
    values
}

fn property_json_value(value: &PropertyValue) -> serde_json::Value {
    match value {
        PropertyValue::String(value) | PropertyValue::Text(value) => {
            serde_json::Value::String(value.clone())
        }
        PropertyValue::Number(value) => lossless_json_number(value)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(value.clone())),
        PropertyValue::Bool(value) => serde_json::Value::Bool(*value),
        PropertyValue::Null => serde_json::Value::Null,
    }
}

fn lossless_json_number(value: &str) -> Option<serde_json::Number> {
    let number = serde_json::from_str::<serde_json::Number>(value).ok()?;
    (number.to_string() == value).then_some(number)
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

fn legacy_log_page_path_in_dir(id: &LogPageId, dir: &Path) -> PathBuf {
    dir.join(format!("{}.log", id.as_str()))
}

fn generation_log_path_in_dir(id: &LogPageId, file_name: &str, dir: &Path) -> Option<PathBuf> {
    const MAX_FILE_NAME_LEN: usize = 255;

    if file_name.is_empty()
        || file_name.len() > MAX_FILE_NAME_LEN
        || !file_name.ends_with(".log")
        || !file_name.starts_with(&format!("{}.", id.as_str()))
        || !file_name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return None;
    }

    let mut components = Path::new(file_name).components();
    let is_single_normal_component = matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(name)), None) if name.to_str() == Some(file_name)
    );
    is_single_normal_component.then(|| dir.join(file_name))
}

static NEXT_PAGE_LOG_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_generation_log_file_name(id: &LogPageId) -> String {
    let sequence = NEXT_PAGE_LOG_GENERATION.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}.{}.{}.{}.log",
        id.as_str(),
        std::process::id(),
        current_unix_nanos(),
        sequence
    )
}

fn next_metadata_publish_path(metadata_path: &Path, dir: &Path) -> PathBuf {
    let sequence = NEXT_PAGE_LOG_GENERATION.fetch_add(1, Ordering::Relaxed);
    let metadata_name = metadata_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("active page metadata has a UTF-8 file name");
    dir.join(format!(
        ".{metadata_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ))
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

fn active_log_page_lock_path_in_dir(id: &LogPageId, dir: &Path) -> PathBuf {
    dir.join(format!("{}.lock", id.as_str()))
}

fn resolve_active_log_page(
    id: &LogPageId,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<ResolvedLogPage, LogPageError> {
    let metadata_path = active_log_page_path_in_dir(id, registry_dir);
    let missing_path = legacy_log_page_path_in_dir(id, page_dir);
    let input = fs::read_to_string(&metadata_path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            LogPageError::MissingPage {
                id: id.clone(),
                path: missing_path.clone(),
            }
        } else {
            LogPageError::Io {
                action: "read active page metadata",
                path: metadata_path.clone(),
                source,
            }
        }
    })?;
    let metadata = serde_json::from_str::<ActiveLogPageMetadata>(&input).map_err(|_| {
        LogPageError::MissingPage {
            id: id.clone(),
            path: missing_path.clone(),
        }
    })?;

    if !metadata.ready || metadata.id != id.as_str() {
        return Err(LogPageError::MissingPage {
            id: id.clone(),
            path: missing_path,
        });
    }

    if metadata.uses_stable_lock() {
        match PageIdLock::try_acquire(id, registry_dir)? {
            Some(_stale_lock) => {
                return Err(LogPageError::MissingPage {
                    id: id.clone(),
                    path: missing_path,
                });
            }
            None => {
                let current = fs::read_to_string(&metadata_path)
                    .ok()
                    .and_then(|input| serde_json::from_str::<ActiveLogPageMetadata>(&input).ok());
                if current.as_ref() != Some(&metadata) {
                    return Err(LogPageError::MissingPage {
                        id: id.clone(),
                        path: missing_path,
                    });
                }
            }
        }
    } else if !process_is_active(metadata.pid) {
        return Err(LogPageError::MissingPage {
            id: id.clone(),
            path: missing_path,
        });
    }
    let log_path = match metadata.log_file.as_deref() {
        Some(file_name) => generation_log_path_in_dir(id, file_name, page_dir),
        None => Some(legacy_log_page_path_in_dir(id, page_dir)),
    }
    .ok_or_else(|| LogPageError::MissingPage {
        id: id.clone(),
        path: legacy_log_page_path_in_dir(id, page_dir),
    })?;

    Ok(ResolvedLogPage { metadata, log_path })
}

fn read_active_page_metadata_for_scan(path: &Path) -> Result<Option<String>, LogPageError> {
    match fs::read_to_string(path) {
        Ok(input) => Ok(Some(input)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LogPageError::Io {
            action: "read active page metadata",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn active_log_pages_from_dirs(
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<Vec<ActiveLogPage>, LogPageError> {
    let entries = match fs::read_dir(registry_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LogPageError::Io {
                action: "read active page directory",
                path: registry_dir.to_path_buf(),
                source,
            });
        }
    };

    let mut pages = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| LogPageError::Io {
            action: "read active page directory entry",
            path: registry_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }

        let Some(input) = read_active_page_metadata_for_scan(&path)? else {
            continue;
        };
        let Ok(metadata) = serde_json::from_str::<ActiveLogPageMetadata>(&input) else {
            continue;
        };

        let Ok(id) = LogPageId::parse(&metadata.id) else {
            continue;
        };
        if active_log_page_path_in_dir(&id, registry_dir) != path {
            continue;
        }

        if !metadata.uses_stable_lock() && process_is_active(metadata.pid) {
            if metadata.ready {
                pages.push(metadata.public_page());
            }
            continue;
        }

        let Some(_lock) = PageIdLock::try_acquire(&id, registry_dir)? else {
            // A current-version session proves liveness by retaining this lock.
            // Re-read after observing the lock so a handoff cannot make stale
            // metadata look like the successor registration.
            if metadata.uses_stable_lock() && metadata.ready {
                let current_input = match fs::read_to_string(&path) {
                    Ok(input) => input,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(source) => {
                        return Err(LogPageError::Io {
                            action: "read locked active page metadata",
                            path: path.clone(),
                            source,
                        });
                    }
                };
                if matches!(
                    serde_json::from_str::<ActiveLogPageMetadata>(&current_input),
                    Ok(current) if current == metadata
                ) {
                    pages.push(metadata.public_page());
                }
            }
            continue;
        };

        // The metadata may have changed between the first read and lock
        // acquisition. Re-read under the stable per-ID lock so an older
        // reaper can never unlink a successor registration.
        let current_input = match fs::read(&path) {
            Ok(input) => input,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(LogPageError::Io {
                    action: "read active page metadata under lock",
                    path: path.clone(),
                    source,
                });
            }
        };
        let current = match serde_json::from_slice::<ActiveLogPageMetadata>(&current_input) {
            Ok(current) if current.id == id.as_str() => current,
            Ok(_) => {
                remove_file_if_exists(&path, "remove mismatched active page metadata")?;
                continue;
            }
            Err(_) => {
                remove_file_if_exists(&path, "remove invalid active page metadata")?;
                continue;
            }
        };

        if !current.uses_stable_lock() && process_is_active(current.pid) {
            if current.ready {
                pages.push(current.public_page());
            }
            continue;
        }
        reap_stale_log_page(&current, &path, page_dir)?;
    }

    pages.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(pages)
}

fn reap_stale_log_page(
    metadata: &ActiveLogPageMetadata,
    metadata_path: &Path,
    page_dir: &Path,
) -> Result<(), LogPageError> {
    if let Ok(id) = LogPageId::parse(&metadata.id) {
        let log_path = match metadata.log_file.as_deref() {
            Some(file_name) => generation_log_path_in_dir(&id, file_name, page_dir),
            None if metadata.ready => Some(legacy_log_page_path_in_dir(&id, page_dir)),
            None => None,
        };
        if let Some(log_path) = log_path {
            remove_file_if_exists(&log_path, "remove stale active page data")?;
        }
    }

    // Metadata is the ID lease: release it only after deleting the exact data
    // generation named by that lease.
    remove_file_if_exists(metadata_path, "remove stale active page metadata")
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

fn current_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
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
    use crate::facet::{FacetKind, FacetOptions};

    fn query_records_from_input(input: &str, options: &LogPageQueryOptions) -> Vec<LogPageRecord> {
        let records = parsed_log_page_records(
            BufReader::new(input.as_bytes()),
            &options.source_config,
            Path::new("test.log"),
        )
        .unwrap();
        let filter = log_filter_for_options(options).unwrap();
        tail_matching_records(&records, &filter, options.line_count)
            .into_iter()
            .map(LogPageRecord::from_parsed)
            .collect()
    }

    fn read_metadata(path: &Path) -> ActiveLogPageMetadata {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn facet_group(groups: &[FacetGroup], kind: FacetKind) -> &FacetGroup {
        groups.iter().find(|group| group.facet == kind).unwrap()
    }

    fn test_metadata(id: &LogPageId, pid: u32, log_file: Option<&str>) -> ActiveLogPageMetadata {
        ActiveLogPageMetadata {
            lease_version: Some(ACTIVE_LOG_PAGE_LEASE_VERSION),
            id: id.as_str().to_string(),
            pid,
            started_unix_seconds: 123,
            command: "docker compose up".to_string(),
            source_fields: Vec::new(),
            log_file: log_file.map(str::to_string),
            ready: true,
        }
    }

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
    fn page_query_composes_source_text_level_and_property_predicates() {
        let input = "api | ERROR database failed tenantId=tenant-1\napi | ERROR database failed tenantId=tenant-1 skip=true\napi | INFO database failed tenantId=tenant-1\nweb | ERROR database failed tenantId=tenant-1\n";
        let options = LogPageQueryOptions {
            line_count: 10,
            source: Some("api".to_string()),
            text: Some("database".to_string()),
            level: Level::parse("ERR"),
            property_filters: vec!["tenantId=tenant-1".to_string(), "!skip".to_string()],
            source_config: SourceConfig::default(),
        };

        let filter = log_filter_for_options(&options).unwrap();
        assert_eq!(filter.level, Some(Level::Error));
        let records = query_records_from_input(input, &options);

        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].raw,
            "api | ERROR database failed tenantId=tenant-1"
        );
    }

    #[test]
    fn structured_records_preserve_types_with_lossless_numeric_fallback() {
        let input = "api | INFO values canonical=500 leading=01 trailing=1. huge=18446744073709551616 flag=true missing=null label=\"ok\" text=bare\n";
        let records = query_records_from_input(input, &LogPageQueryOptions::new(10));

        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.schema_version, 1);
        assert_eq!(record.source, "api");
        assert_eq!(record.timestamp, None);
        assert_eq!(record.level, Level::Info);
        assert_eq!(record.properties["canonical"], serde_json::json!(500));
        assert_eq!(record.properties["leading"], serde_json::json!("01"));
        assert_eq!(record.properties["trailing"], serde_json::json!("1."));
        assert_eq!(
            record.properties["huge"],
            serde_json::json!("18446744073709551616")
        );
        assert_eq!(record.properties["flag"], serde_json::json!(true));
        assert_eq!(record.properties["missing"], serde_json::Value::Null);
        assert_eq!(record.properties["label"], serde_json::json!("ok"));
        assert_eq!(record.properties["text"], serde_json::json!("bare"));
    }

    #[test]
    fn structured_record_has_canonical_level_and_complete_multiline_raw() {
        let input = "api | 14:06:58.892 ERROR request failed\n[14:06:58.892] ERROR (#1):\n  {\n    retryable: true,\n    statusCode: 500,\n  }\n";
        let records = query_records_from_input(input, &LogPageQueryOptions::new(10));

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp.as_deref(), Some("14:06:58.892"));
        assert_eq!(records[0].level, Level::Error);
        assert_eq!(records[0].message, "request failed");
        assert_eq!(records[0].properties["retryable"], serde_json::json!(true));
        assert_eq!(records[0].properties["statusCode"], serde_json::json!(500));
        assert_eq!(records[0].raw, input.trim_end());

        let json = serde_json::to_string(&records[0]).unwrap();
        assert_eq!(json.lines().count(), 1);
        assert!(json.contains("\"level\":\"error\""));
        assert!(!json.contains("sequence"));
    }

    #[test]
    fn structured_record_uses_first_duplicate_property() {
        let input = "api | INFO request tenantId=first tenantId=second\n";
        let records = query_records_from_input(input, &LogPageQueryOptions::new(10));

        assert_eq!(
            records[0].properties["tenantId"],
            serde_json::json!("first")
        );
    }

    #[test]
    fn structured_query_returns_last_matching_records_in_original_order() {
        let input =
            "api | ERROR first\napi | INFO ignored\napi | ERROR second\napi | ERROR third\n";
        let mut options = LogPageQueryOptions::new(2);
        options.level = Some(Level::Error);

        let records = query_records_from_input(input, &options);

        assert_eq!(
            records
                .iter()
                .map(|record| record.message.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "third"]
        );
    }

    #[test]
    fn structured_query_returns_empty_for_no_matches() {
        let input = "api | INFO ready\n";
        let mut options = LogPageQueryOptions::new(10);
        options.level = Some(Level::Error);

        assert!(query_records_from_input(input, &options).is_empty());
    }

    #[test]
    fn filtered_tail_lines_includes_property_block_that_precedes_summary() {
        let input = "[api] [21:05:37.312] INFO (#140):\n[api] {\n[api] requestId: \"abc-123\",\n[api] statusCode: 200,\n[api] }\n[api] 21:05:37.312 INFO http.request ok\n"
            .as_bytes();
        let options = LogPageTailOptions {
            line_count: 5,
            source: None,
            text: None,
            property_filters: vec!["requestId=abc-123".to_string()],
            source_config: SourceConfig::default(),
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        assert_eq!(
            lines,
            vec![
                "[api] [21:05:37.312] INFO (#140):".to_string(),
                "[api] {".to_string(),
                "[api] requestId: \"abc-123\",".to_string(),
                "[api] statusCode: 200,".to_string(),
                "[api] }".to_string(),
                "[api] 21:05:37.312 INFO http.request ok".to_string(),
            ]
        );
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
    fn public_page_read_rejects_orphan_log_data() {
        let root = env::temp_dir().join(format!(
            "loggle-orphan-page-read-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let data_path = legacy_log_page_path_in_dir(&id, &page_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(&data_path, "stale secret\n").unwrap();

        let mut output = Vec::new();
        let error = print_log_page_tail_from_dirs(
            &id,
            &LogPageTailOptions::new(10),
            &mut output,
            &registry_dir,
            &page_dir,
        )
        .unwrap_err();

        assert!(matches!(error, LogPageError::MissingPage { .. }));
        assert!(output.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn public_page_read_does_not_reap_dead_session_files() {
        let root =
            env::temp_dir().join(format!("loggle-dead-page-read-test-{}", std::process::id()));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let log_file = "api.0.dead.1.log";
        let data_path = page_dir.join(log_file);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let metadata = test_metadata(&id, 0, Some(log_file));
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        fs::write(&data_path, "stale data\n").unwrap();

        let mut output = Vec::new();
        let error = print_log_page_tail_from_dirs(
            &id,
            &LogPageTailOptions::new(10),
            &mut output,
            &registry_dir,
            &page_dir,
        )
        .unwrap_err();

        assert!(matches!(error, LogPageError::MissingPage { .. }));
        assert!(metadata_path.is_file());
        assert_eq!(fs::read_to_string(&data_path).unwrap(), "stale data\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_page_read_is_bound_to_metadata_generation() {
        let root = env::temp_dir().join(format!(
            "loggle-generation-bound-read-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let old_log_file = "api.1.old.1.log";
        let old_log_path = page_dir.join(old_log_file);
        let successor_log_file = "api.2.new.2.log";
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let _lock = PageIdLock::try_acquire(&id, &registry_dir)
            .unwrap()
            .unwrap();
        let metadata = test_metadata(&id, std::process::id(), Some(old_log_file));
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        fs::write(old_log_path, "metadata generation\n").unwrap();
        fs::write(page_dir.join(successor_log_file), "successor generation\n").unwrap();
        fs::write(
            legacy_log_page_path_in_dir(&id, &page_dir),
            "successor shared path\n",
        )
        .unwrap();

        let mut output = Vec::new();
        print_log_page_tail_from_dirs(
            &id,
            &LogPageTailOptions::new(10),
            &mut output,
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "metadata generation\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_page_read_rejects_unsafe_generation_file_name() {
        let root = env::temp_dir().join(format!(
            "loggle-unsafe-generation-read-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let _lock = PageIdLock::try_acquire(&id, &registry_dir)
            .unwrap()
            .unwrap();
        let metadata = test_metadata(&id, std::process::id(), Some("../secret.log"));
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

        let mut output = Vec::new();
        let error = print_log_page_tail_from_dirs(
            &id,
            &LogPageTailOptions::new(10),
            &mut output,
            &registry_dir,
            &page_dir,
        )
        .unwrap_err();

        assert!(matches!(error, LogPageError::MissingPage { .. }));
        assert!(output.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_page_read_inherits_session_source_fields() {
        let root = env::temp_dir().join(format!(
            "loggle-inherited-page-source-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let source_config = SourceConfig::with_fields(["tenant"]);
        let _ = fs::remove_dir_all(&root);
        let mut session = PageLogSession::start_in_dirs(
            Some(id.clone()),
            "docker compose up",
            &source_config,
            100,
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        session.record_line("INFO ready tenant=api").unwrap();
        session.flush().unwrap();

        let mut options = LogPageTailOptions::new(10);
        options.source = Some("api".to_string());
        let mut output = Vec::new();
        print_log_page_tail_from_dirs(&id, &options, &mut output, &registry_dir, &page_dir)
            .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "INFO ready tenant=api\n"
        );
        drop(session);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_page_read_prefers_reader_source_fields() {
        let root = env::temp_dir().join(format!(
            "loggle-reader-page-source-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let source_config = SourceConfig::with_fields(["session"]);
        let _ = fs::remove_dir_all(&root);
        let mut session = PageLogSession::start_in_dirs(
            Some(id.clone()),
            "docker compose up",
            &source_config,
            100,
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        session
            .record_line("INFO ready reader=cli session=server")
            .unwrap();
        session.flush().unwrap();

        let mut options = LogPageTailOptions::new(10);
        options.source = Some("cli".to_string());
        options.source_config = SourceConfig::with_fields(["reader"]);
        let mut output = Vec::new();
        print_log_page_tail_from_dirs(&id, &options, &mut output, &registry_dir, &page_dir)
            .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "INFO ready reader=cli session=server\n"
        );
        drop(session);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facet_query_validates_record_window_before_scanning() {
        let id = LogPageId::parse("missing").unwrap();
        let registry_dir = Path::new("definitely-missing-registry");
        let page_dir = Path::new("definitely-missing-pages");

        for value in [0, MAX_FACET_RECORD_LIMIT + 1] {
            let error = query_log_page_facets_from_dirs(
                &id,
                &LogPageQueryOptions::new(value),
                &FacetOptions::default(),
                registry_dir,
                page_dir,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                FacetQueryError::InvalidRecordWindow { value: actual } if actual == value
            ));
        }

        let error = query_log_page_facets_from_dirs(
            &id,
            &LogPageQueryOptions::new(0),
            &FacetOptions::default(),
            registry_dir,
            page_dir,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "facet record window 0 is outside 1..=100000"
        );
    }

    #[test]
    fn facet_query_propagates_active_page_errors() {
        let root = env::temp_dir().join(format!(
            "loggle-facet-orphan-page-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(legacy_log_page_path_in_dir(&id, &page_dir), "secret\n").unwrap();

        let error = query_log_page_facets_from_dirs(
            &id,
            &LogPageQueryOptions::new(10),
            &FacetOptions::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            FacetQueryError::Page(LogPageError::MissingPage { .. })
        ));
        assert!(std::error::Error::source(&error).is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facet_query_inherits_and_overrides_session_source_fields() {
        let root = env::temp_dir().join(format!(
            "loggle-facet-source-fields-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let source_config = SourceConfig::with_fields(["tenant"]);
        let _ = fs::remove_dir_all(&root);
        let mut session = PageLogSession::start_in_dirs(
            Some(id.clone()),
            "docker compose up",
            &source_config,
            100,
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        session
            .record_line("INFO ready reader=cli tenant=server")
            .unwrap();
        session.flush().unwrap();

        let mut inherited = LogPageQueryOptions::new(10);
        inherited.source = Some("server".to_string());
        let inherited_groups = query_log_page_facets_from_dirs(
            &id,
            &inherited,
            &FacetOptions::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        assert_eq!(
            facet_group(&inherited_groups, FacetKind::PropertyKey).matched_records,
            1
        );

        let mut overridden = LogPageQueryOptions::new(10);
        overridden.source = Some("cli".to_string());
        overridden.source_config = SourceConfig::with_fields(["reader"]);
        let overridden_groups = query_log_page_facets_from_dirs(
            &id,
            &overridden,
            &FacetOptions::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        assert_eq!(
            facet_group(&overridden_groups, FacetKind::PropertyKey).matched_records,
            1
        );

        drop(session);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facet_query_fixes_newest_logical_window_before_filters() {
        let root = env::temp_dir().join(format!(
            "loggle-facet-newest-window-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let _ = fs::remove_dir_all(&root);
        let mut session = PageLogSession::start_in_dirs(
            Some(id.clone()),
            "docker compose up",
            &SourceConfig::default(),
            100,
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        session.record_line("old | ERROR wanted tenant=t1").unwrap();
        session.record_line("web | INFO ignored tenant=t2").unwrap();
        session.record_line("api | ERROR wanted tenant=t1").unwrap();
        session.flush().unwrap();

        let mut options = LogPageQueryOptions::new(2);
        options.text = Some("wanted".to_string());
        let groups = query_log_page_facets_from_dirs(
            &id,
            &options,
            &FacetOptions::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        let source = facet_group(&groups, FacetKind::Source);
        assert_eq!(source.available_records, 3);
        assert_eq!(source.window_records, 2);
        assert!(source.window_truncated);
        assert_eq!(source.matched_records, 1);
        assert_eq!(source.buckets[0].value, "api");

        drop(session);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facet_query_composes_every_query_filter_dimension() {
        let root = env::temp_dir().join(format!(
            "loggle-facet-filter-composition-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let _ = fs::remove_dir_all(&root);
        let mut session = PageLogSession::start_in_dirs(
            Some(id.clone()),
            "docker compose up",
            &SourceConfig::default(),
            100,
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        for line in [
            "api | ERROR database tenant=t1 region=eu",
            "web | ERROR database tenant=t1 region=eu",
            "api | INFO database tenant=t1 region=eu",
            "api | ERROR other tenant=t1 region=eu",
            "api | ERROR database tenant=t2 region=eu",
            "api | ERROR database tenant=t1 region=us",
        ] {
            session.record_line(line).unwrap();
        }
        session.flush().unwrap();

        let mut options = LogPageQueryOptions::new(100);
        options.source = Some("api".to_string());
        options.text = Some("database".to_string());
        options.level = Some(Level::Error);
        options.property_filters = vec!["tenant=t1".to_string(), "region=eu".to_string()];
        let facet_options = FacetOptions::new(20, Some("tenant".to_string())).unwrap();
        let groups = query_log_page_facets_from_dirs(
            &id,
            &options,
            &facet_options,
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        assert!(groups.iter().all(|group| group.matched_records == 1));
        assert_eq!(facet_group(&groups, FacetKind::Source).eligible_records, 2);
        assert_eq!(facet_group(&groups, FacetKind::Level).eligible_records, 2);
        assert_eq!(
            facet_group(&groups, FacetKind::PropertyValue).eligible_records,
            2
        );

        drop(session);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_page_read_uses_defaults_for_legacy_metadata() {
        let root = env::temp_dir().join(format!(
            "loggle-legacy-page-source-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let data_path = legacy_log_page_path_in_dir(&id, &page_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(
            metadata_path,
            format!(
                r#"{{
                    "id": "api",
                    "pid": {},
                    "started_unix_seconds": 123,
                    "command": "docker compose up"
                }}"#,
                std::process::id()
            ),
        )
        .unwrap();
        fs::write(data_path, "INFO ready service=api\n").unwrap();

        let pages = active_log_pages_from_dirs(&registry_dir, &page_dir).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].id, "api");
        assert_eq!(pages[0].pid, std::process::id());

        let mut options = LogPageTailOptions::new(10);
        options.source = Some("api".to_string());
        let mut output = Vec::new();
        print_log_page_tail_from_dirs(&id, &options, &mut output, &registry_dir, &page_dir)
            .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "INFO ready service=api\n"
        );
        let _ = fs::remove_dir_all(root);
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
    fn page_log_session_drop_removes_registration_and_data() {
        let root = env::temp_dir().join(format!(
            "loggle-page-session-drop-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let unrelated_path = page_dir.join("api.999.unrelated.1.log");
        let data_path;
        let _ = fs::remove_dir_all(&root);

        {
            let mut session = PageLogSession::start_in_dirs(
                Some(id.clone()),
                "docker compose up",
                &SourceConfig::default(),
                100,
                &registry_dir,
                &page_dir,
            )
            .unwrap();
            session.record_line("api | ready").unwrap();
            session.flush().unwrap();

            assert!(metadata_path.is_file());
            let metadata = read_metadata(&metadata_path);
            assert!(metadata.ready);
            let log_file = metadata.log_file.as_deref().unwrap();
            data_path = generation_log_path_in_dir(&id, log_file, &page_dir).unwrap();
            assert_eq!(fs::read_to_string(&data_path).unwrap(), "api | ready\n");
            fs::write(&unrelated_path, "unrelated\n").unwrap();
        }

        assert!(!metadata_path.exists());
        assert!(!data_path.exists());
        assert_eq!(fs::read_to_string(&unrelated_path).unwrap(), "unrelated\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_log_page_session_keeps_private_metadata_out_of_public_listing() {
        let root = env::temp_dir().join(format!("loggle-active-page-test-{}", std::process::id()));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let _ = fs::remove_dir_all(&root);
        let id = LogPageId::parse("api").unwrap();
        let source_config = SourceConfig::with_fields(["tenant", "service"]);

        {
            let session = PageLogSession::start_in_dirs(
                Some(id.clone()),
                "docker compose up",
                &source_config,
                100,
                &registry_dir,
                &page_dir,
            )
            .unwrap();
            let pages = active_log_pages_from_dirs(&registry_dir, &page_dir).unwrap();

            assert_eq!(
                pages,
                vec![ActiveLogPage {
                    id: "api".to_string(),
                    pid: std::process::id(),
                    started_unix_seconds: pages[0].started_unix_seconds,
                    command: "docker compose up".to_string(),
                }]
            );
            let public_json = serde_json::to_value(&pages[0]).unwrap();
            let public_object = public_json.as_object().unwrap();
            assert_eq!(public_object.len(), 4);
            assert!(!public_object.contains_key("source_fields"));
            assert!(!public_object.contains_key("log_file"));

            let metadata = read_metadata(&active_log_page_path_in_dir(&id, &registry_dir));
            assert!(metadata.ready);
            assert_eq!(metadata.source_fields, ["tenant", "service"]);
            assert!(metadata.log_file.is_some());
            drop(session);
        }

        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn private_metadata_decodes_legacy_registration_defaults() {
        let metadata: ActiveLogPageMetadata = serde_json::from_str(
            r#"{
                "id": "api",
                "pid": 42,
                "started_unix_seconds": 123,
                "command": "docker compose up"
            }"#,
        )
        .unwrap();

        assert!(metadata.source_fields.is_empty());
        assert!(metadata.log_file.is_none());
        assert!(metadata.ready);
        assert_eq!(metadata.lease_version, None);
        assert_eq!(
            metadata.public_page(),
            ActiveLogPage {
                id: "api".to_string(),
                pid: 42,
                started_unix_seconds: 123,
                command: "docker compose up".to_string(),
            }
        );
    }

    #[test]
    fn active_log_pages_ignore_invalid_metadata_files() {
        let root = env::temp_dir().join(format!(
            "loggle-invalid-active-page-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(registry_dir.join("invalid.json"), "{").unwrap();

        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_page_scan_treats_disappeared_metadata_as_absent() {
        let root = env::temp_dir().join(format!(
            "loggle-disappeared-active-page-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        let metadata_path = registry_dir.join("api.json");
        fs::write(&metadata_path, "{}").unwrap();
        let enumerated_path = fs::read_dir(&registry_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::remove_file(&metadata_path).unwrap();

        assert_eq!(
            read_active_page_metadata_for_scan(&enumerated_path).unwrap(),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claiming_page_binds_pending_generation_before_data_creation() {
        let root = env::temp_dir().join(format!(
            "loggle-pending-page-claim-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let _ = fs::remove_dir_all(&root);

        let (_, registration) = claim_active_log_page_in_dirs(
            Some(id.clone()),
            "docker compose up",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        let metadata = read_metadata(&metadata_path);
        assert!(!metadata.ready);
        let log_file = metadata.log_file.as_deref().unwrap();
        let log_path = generation_log_path_in_dir(&id, log_file, &page_dir).unwrap();
        assert!(!log_path.exists());
        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        drop(registration);
        assert!(!metadata_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn stale_pending_generation_and_orphan_data_are_reaped_together() {
        let root = env::temp_dir().join(format!(
            "loggle-pending-orphan-page-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let log_file = "api.0.pending.1.log";
        let log_path = page_dir.join(log_file);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let mut metadata = test_metadata(&id, std::process::id(), Some(log_file));
        metadata.ready = false;
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        fs::write(&log_path, "orphaned pending data\n").unwrap();

        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        assert!(!metadata_path.exists());
        assert!(!log_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generation_collision_preserves_existing_data_and_releases_lease() {
        let root = env::temp_dir().join(format!(
            "loggle-generation-collision-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let requested = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&requested, &registry_dir);
        let _ = fs::remove_dir_all(&root);

        let (id, registration) = claim_active_log_page_in_dirs(
            Some(requested),
            "docker compose up",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        let metadata = read_metadata(&metadata_path);
        let log_path =
            generation_log_path_in_dir(&id, metadata.log_file.as_deref().unwrap(), &page_dir)
                .unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(&log_path, "pre-existing collision\n").unwrap();

        let error = match PageLogSession::from_claim(id, registration, &page_dir, 100) {
            Ok(_) => panic!("pre-existing generation must not be opened"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            LogPageError::Io { source, .. }
                if source.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(
            fs::read_to_string(&log_path).unwrap(),
            "pre-existing collision\n"
        );
        assert!(!metadata_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorder_creation_failure_releases_unpublished_page_lease() {
        let root = env::temp_dir().join(format!(
            "loggle-recorder-create-failure-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(&page_dir, "not a directory").unwrap();

        let result = PageLogSession::start_in_dirs(
            Some(id),
            "docker compose up",
            &SourceConfig::default(),
            100,
            &registry_dir,
            &page_dir,
        );

        assert!(matches!(result, Err(LogPageError::Io { .. })));
        assert!(!metadata_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn stale_reaper_uses_lock_not_reused_pid_and_removes_only_its_generation() {
        let root = env::temp_dir().join(format!(
            "loggle-generation-reaper-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let old_log_file = "api.0.old.1.log";
        let old_log_path = page_dir.join(old_log_file);
        let successor_path = page_dir.join("api.1.successor.2.log");
        let legacy_path = legacy_log_page_path_in_dir(&id, &page_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        // A reused PID must not keep a versioned lease alive after its lock is
        // gone.
        let metadata = test_metadata(&id, std::process::id(), Some(old_log_file));
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        fs::write(&old_log_path, "old\n").unwrap();
        fs::write(&successor_path, "successor\n").unwrap();
        fs::write(&legacy_path, "legacy\n").unwrap();

        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );

        assert!(!old_log_path.exists());
        assert!(!metadata_path.exists());
        assert_eq!(fs::read_to_string(successor_path).unwrap(), "successor\n");
        assert_eq!(fs::read_to_string(legacy_path).unwrap(), "legacy\n");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn stable_lock_ownership_controls_versioned_reaping() {
        let root =
            env::temp_dir().join(format!("loggle-locked-reaper-test-{}", std::process::id()));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let log_file = "api.0.locked.1.log";
        let log_path = page_dir.join(log_file);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let lock = PageIdLock::try_acquire(&id, &registry_dir)
            .unwrap()
            .unwrap();
        let metadata = test_metadata(&id, 0, Some(log_file));
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        fs::write(&log_path, "old\n").unwrap();

        let pages = active_log_pages_from_dirs(&registry_dir, &page_dir).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].id, "api");
        assert!(metadata_path.is_file());
        assert!(log_path.is_file());

        drop(lock);
        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        assert!(!metadata_path.exists());
        assert!(!log_path.exists());
        assert!(
            active_log_pages_from_dirs(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claim_cleans_invalid_metadata_before_creating_pending_lease() {
        let root = env::temp_dir().join(format!(
            "loggle-invalid-page-claim-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(&metadata_path, b"{\xff").unwrap();

        let registration = try_register_active_log_page(
            &id,
            "replacement",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        let metadata = read_metadata(&metadata_path);
        assert_eq!(metadata.lease_version, Some(ACTIVE_LOG_PAGE_LEASE_VERSION));
        assert!(!metadata.ready);
        assert_eq!(metadata.command, "replacement");
        drop(registration);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn claim_replaces_dead_legacy_metadata_and_data() {
        let root = env::temp_dir().join(format!(
            "loggle-dead-legacy-page-claim-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let legacy_path = legacy_log_page_path_in_dir(&id, &page_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(
            &metadata_path,
            r#"{
                "id": "api",
                "pid": 0,
                "started_unix_seconds": 123,
                "command": "legacy"
            }"#,
        )
        .unwrap();
        fs::write(&legacy_path, "legacy data\n").unwrap();

        let registration = try_register_active_log_page(
            &id,
            "replacement",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        assert!(!legacy_path.exists());
        let metadata = read_metadata(&metadata_path);
        assert_eq!(metadata.lease_version, Some(ACTIVE_LOG_PAGE_LEASE_VERSION));
        assert!(!metadata.ready);
        drop(registration);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claim_preserves_active_legacy_metadata() {
        let root = env::temp_dir().join(format!(
            "loggle-active-legacy-page-claim-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let legacy_path = legacy_log_page_path_in_dir(&id, &page_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let legacy_json = format!(
            r#"{{
                "id": "api",
                "pid": {},
                "started_unix_seconds": 123,
                "command": "legacy"
            }}"#,
            std::process::id()
        );
        fs::write(&metadata_path, &legacy_json).unwrap();
        fs::write(&legacy_path, "live legacy data\n").unwrap();

        let error = try_register_active_log_page(
            &id,
            "replacement",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap_err();

        assert!(matches!(error, LogPageError::ActivePageIdInUse(_)));
        assert_eq!(fs::read_to_string(&metadata_path).unwrap(), legacy_json);
        assert_eq!(
            fs::read_to_string(&legacy_path).unwrap(),
            "live legacy data\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn claim_replaces_versioned_lease_despite_reused_pid() {
        let root = env::temp_dir().join(format!(
            "loggle-reused-pid-page-claim-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let old_log_file = "api.1.old.1.log";
        let old_log_path = page_dir.join(old_log_file);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        let metadata = test_metadata(&id, std::process::id(), Some(old_log_file));
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        fs::write(&old_log_path, "stale generation\n").unwrap();

        let registration = try_register_active_log_page(
            &id,
            "replacement",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        assert!(!old_log_path.exists());
        let replacement = read_metadata(&metadata_path);
        assert_eq!(
            replacement.lease_version,
            Some(ACTIVE_LOG_PAGE_LEASE_VERSION)
        );
        assert!(!replacement.ready);
        assert_eq!(replacement.command, "replacement");
        drop(registration);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claim_active_log_page_allocates_first_available_numeric_id() {
        let root = env::temp_dir().join(format!(
            "loggle-allocate-page-id-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let _ = fs::remove_dir_all(&root);
        let one = LogPageId::parse("1").unwrap();
        let three = LogPageId::parse("3").unwrap();
        let source_config = SourceConfig::default();
        let (_, _one) = claim_active_log_page_in_dirs(
            Some(one),
            "one",
            &source_config,
            &registry_dir,
            &page_dir,
        )
        .unwrap();
        let (_, _three) = claim_active_log_page_in_dirs(
            Some(three),
            "three",
            &source_config,
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        let (id, _registration) =
            claim_active_log_page_in_dirs(None, "two", &source_config, &registry_dir, &page_dir)
                .unwrap();

        assert_eq!(id.as_str(), "2");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn requested_page_id_bypasses_unrelated_broken_registry_entry() {
        let root = env::temp_dir().join(format!(
            "loggle-requested-page-direct-claim-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let id = LogPageId::parse("api").unwrap();
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(registry_dir.join("unrelated.json")).unwrap();

        let (_, registration) = claim_active_log_page_in_dirs(
            Some(id),
            "api",
            &SourceConfig::default(),
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        assert!(metadata_path.is_file());
        drop(registration);
        assert!(!metadata_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claim_active_log_page_rejects_active_requested_id() {
        let root = env::temp_dir().join(format!(
            "loggle-requested-page-id-test-{}",
            std::process::id()
        ));
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        let _ = fs::remove_dir_all(&root);
        let id = LogPageId::parse("api").unwrap();
        let source_config = SourceConfig::default();
        let (_, _registration) = claim_active_log_page_in_dirs(
            Some(id.clone()),
            "api",
            &source_config,
            &registry_dir,
            &page_dir,
        )
        .unwrap();

        let error = claim_active_log_page_in_dirs(
            Some(id),
            "api",
            &source_config,
            &registry_dir,
            &page_dir,
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "log page id 'api' is already active");
        let _ = fs::remove_dir_all(root);
    }
}
