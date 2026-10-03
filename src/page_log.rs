use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    env, fmt, fs,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize, Serializer};

pub use crate::model::Level as LogLevel;
use crate::{
    buffer::LogBuffer,
    facet::{
        DEFAULT_FACET_BUCKET_LIMIT, DEFAULT_FACET_RECORD_LIMIT, FacetGroup, FacetKind,
        FacetOptions, FacetOptionsError, MAX_FACET_BUCKET_LIMIT, MAX_FACET_RECORD_LIMIT,
        MIN_FACET_BUCKET_LIMIT, MIN_FACET_RECORD_LIMIT, aggregate_facets, escape_facet_text,
    },
    filter::{LogFilter, PropertyFilterUpdate},
    model::{LogEvent, LogProperty, PropertyValue, SourceConfig, clean_display_text},
};

/// Version of the JSONL record contract emitted by `loggle log --json` and
/// `loggle pages --json`. Bump it on any incompatible change to a record shape.
pub const LOG_PAGE_SCHEMA_VERSION: u32 = 1;

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
    InvalidFacetOptions(FacetOptionsError),
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
            Self::InvalidFacetOptions(source) => write!(f, "invalid facet options: {source}"),
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
            Self::InvalidFacetOptions(source) => Some(source),
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

/// How `loggle log` writes the records it selects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogOutputFormat {
    /// The raw page-log lines, one per output line.
    #[default]
    Text,
    /// One [`LogPageRecord`] JSON object per line (JSONL).
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPageTailOptions {
    pub line_count: usize,
    pub clean: bool,
    pub source: Option<String>,
    pub text: Option<String>,
    pub level: Option<LogLevel>,
    pub property_filters: Vec<String>,
    pub source_config: SourceConfig,
    pub format: LogOutputFormat,
}

impl LogPageTailOptions {
    pub fn new(line_count: usize) -> Self {
        Self {
            line_count,
            clean: false,
            source: None,
            text: None,
            level: None,
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
            format: LogOutputFormat::Text,
        }
    }

    fn has_filters(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| !source.is_empty())
            || self.text.as_ref().is_some_and(|text| !text.is_empty())
            || self.level.is_some()
            || !self.property_filters.is_empty()
    }
}

/// Options for `loggle facets`: the same narrowing filters as
/// [`LogPageTailOptions`], plus which facets to aggregate and their bounds.
///
/// Each facet excludes its own filter (a `level` filter narrows the `source`
/// facet but not the `level` facet), matching the TUI facet dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPageFacetOptions {
    /// Newest parsed records to aggregate; clamped to
    /// `MIN_FACET_RECORD_LIMIT..=MAX_FACET_RECORD_LIMIT`.
    pub record_limit: usize,
    /// Buckets kept per facet; clamped to
    /// `MIN_FACET_BUCKET_LIMIT..=MAX_FACET_BUCKET_LIMIT`.
    pub bucket_limit: usize,
    /// Facets to print. Empty means `source`, `level` and `property_key`.
    /// `property_value` is always included when `property_key` is set, and
    /// is never produced without one.
    pub facets: Vec<FacetKind>,
    /// Property whose values the `property_value` facet counts.
    pub property_key: Option<String>,
    pub source: Option<String>,
    pub text: Option<String>,
    pub level: Option<LogLevel>,
    pub property_filters: Vec<String>,
    pub source_config: SourceConfig,
    pub format: LogOutputFormat,
}

impl Default for LogPageFacetOptions {
    fn default() -> Self {
        Self {
            record_limit: DEFAULT_FACET_RECORD_LIMIT,
            bucket_limit: DEFAULT_FACET_BUCKET_LIMIT,
            facets: Vec::new(),
            property_key: None,
            source: None,
            text: None,
            level: None,
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
            format: LogOutputFormat::Text,
        }
    }
}

impl LogPageFacetOptions {
    fn includes(&self, facet: FacetKind) -> bool {
        match facet {
            FacetKind::PropertyValue => self.property_key.is_some(),
            facet if self.facets.is_empty() => {
                matches!(
                    facet,
                    FacetKind::Source | FacetKind::Level | FacetKind::PropertyKey
                )
            }
            facet => self.facets.contains(&facet),
        }
    }
}

/// One parsed record of a page log, as emitted by `loggle log --json`.
///
/// This is the `schema_version: 1` contract. `sequence` is the record's event
/// sequence within the page's retained window as parsed at query time: it
/// increases in recording order (with gaps where a folded property block
/// consumed a number) and is stable across repeated queries of a page until
/// its log rotates, but it is not the live viewer's internal sequence. `properties` keeps the first value
/// of each key; numbers are JSON numbers only when that is lossless, booleans
/// and null are typed, and everything else is a string. `raw` is the whole
/// record (header plus any folded property block) joined with `\n`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogPageRecord {
    pub schema_version: u32,
    pub sequence: u64,
    pub source: String,
    pub timestamp: Option<String>,
    #[serde(serialize_with = "serialize_level")]
    pub level: LogLevel,
    pub message: String,
    pub properties: BTreeMap<String, serde_json::Value>,
    pub raw: String,
}

impl LogPageRecord {
    fn new(event: &LogEvent, lines: &[String], clean: bool) -> Self {
        let raw = if clean {
            lines
                .iter()
                .map(|line| clean_display_text(line))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            lines.join("\n")
        };
        Self {
            schema_version: LOG_PAGE_SCHEMA_VERSION,
            sequence: event.sequence,
            source: event.source.clone(),
            timestamp: event.timestamp.clone(),
            level: event.level,
            message: event.message.clone(),
            properties: record_properties(&event.properties),
            raw,
        }
    }
}

fn serialize_level<S: Serializer>(level: &LogLevel, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(level.as_str())
}

/// One active page, as emitted by `loggle pages --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveLogPageRecord<'a> {
    pub schema_version: u32,
    #[serde(flatten)]
    pub page: &'a ActiveLogPage,
}

impl<'a> ActiveLogPageRecord<'a> {
    pub fn new(page: &'a ActiveLogPage) -> Self {
        Self {
            schema_version: LOG_PAGE_SCHEMA_VERSION,
            page,
        }
    }
}

/// Writes `value` as one compact JSON object followed by a newline.
pub fn write_json_line<W: Write, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), LogPageError> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| LogPageError::Output(source.into()))?;
    writeln!(writer).map_err(LogPageError::Output)
}

pub fn active_log_pages() -> Result<Vec<ActiveLogPage>, LogPageError> {
    active_log_pages_from_dir(&log_page_registry_dir(), &log_page_dir())
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

/// List observed sources in the retained page, not Compose service names.
pub fn print_log_page_sources<W: Write>(
    id: &LogPageId,
    source_config: SourceConfig,
    writer: &mut W,
) -> Result<(), LogPageError> {
    let path = log_page_path(id);
    let file = open_log_page(id, &path)?;
    let sources =
        source_counts(BufReader::new(file), source_config).map_err(|source| LogPageError::Io {
            action: "read log page",
            path,
            source,
        })?;
    writeln!(writer, "SOURCE\tRECORDS").map_err(LogPageError::Output)?;
    for (source, count) in sources {
        writeln!(writer, "{source}\t{count}").map_err(LogPageError::Output)?;
    }
    Ok(())
}

/// Aggregates source/level/property facets over the newest records of a page,
/// after applying the same narrowing filters as `loggle log`.
///
/// Text output prints one block per facet; JSON output prints one
/// [`FacetGroup`] per line in `source`, `level`, `property_key`,
/// `property_value` order.
pub fn print_log_page_facets<W: Write>(
    id: &LogPageId,
    options: &LogPageFacetOptions,
    writer: &mut W,
) -> Result<(), LogPageError> {
    let path = log_page_path(id);
    print_log_page_facets_from_path(id, &path, options, writer)
}

fn print_log_page_facets_from_path<W: Write>(
    id: &LogPageId,
    path: &Path,
    options: &LogPageFacetOptions,
    writer: &mut W,
) -> Result<(), LogPageError> {
    let file = open_log_page(id, path)?;
    let groups = facet_groups(BufReader::new(file), options, path)?;
    match options.format {
        LogOutputFormat::Json => groups
            .iter()
            .try_for_each(|group| write_json_line(writer, group)),
        LogOutputFormat::Text => write_facet_text(writer, &groups).map_err(LogPageError::Output),
    }
}

fn facet_groups<R: BufRead>(
    reader: R,
    options: &LogPageFacetOptions,
    path: &Path,
) -> Result<Vec<FacetGroup>, LogPageError> {
    let property_key = options
        .property_key
        .as_ref()
        .map(|key| key.trim().to_string());
    let facet_options = FacetOptions::new(
        options
            .bucket_limit
            .clamp(MIN_FACET_BUCKET_LIMIT, MAX_FACET_BUCKET_LIMIT),
        property_key,
    )
    .map_err(LogPageError::InvalidFacetOptions)?;
    let filter = build_log_filter(
        options.source.as_deref(),
        options.text.as_deref(),
        options.level,
        &options.property_filters,
    )?;
    let buffer = parse_log_page(reader, options.source_config.clone()).map_err(|source| {
        LogPageError::Io {
            action: "read log page",
            path: path.to_path_buf(),
            source,
        }
    })?;

    let mut groups = aggregate_facets(
        buffer.events().iter(),
        options
            .record_limit
            .clamp(MIN_FACET_RECORD_LIMIT, MAX_FACET_RECORD_LIMIT),
        &filter,
        &facet_options,
    );
    groups.retain(|group| options.includes(group.facet));
    Ok(groups)
}

/// Writes one block per facet: a heading such as
/// `source (12 records, 3 buckets)` followed by aligned `  value  count` rows.
/// Values are escaped so newlines and control characters stay on one row.
fn write_facet_text<W: Write>(writer: &mut W, groups: &[FacetGroup]) -> io::Result<()> {
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        write!(writer, "{}", group.facet.as_str())?;
        if let Some(key) = &group.property_key {
            write!(writer, " {}", escape_facet_text(key))?;
        }
        write!(
            writer,
            " ({} records, {} buckets",
            group.eligible_records, group.total_buckets
        )?;
        if group.truncated {
            write!(writer, ", top {} shown", group.buckets.len())?;
        }
        if group.window_truncated {
            write!(
                writer,
                ", newest {} of {} records scanned",
                group.window_records, group.available_records
            )?;
        }
        writeln!(writer, ")")?;

        if group.buckets.is_empty() {
            writeln!(writer, "  (none)")?;
            continue;
        }
        let rows = group
            .buckets
            .iter()
            .map(|bucket| (escape_facet_text(&bucket.value), bucket.count.to_string()))
            .collect::<Vec<_>>();
        let value_width = rows
            .iter()
            .map(|(value, _)| value.chars().count())
            .max()
            .unwrap_or(0);
        let count_width = rows.iter().map(|(_, count)| count.len()).max().unwrap_or(0);
        for (value, count) in rows {
            let padding = value_width - value.chars().count();
            writeln!(writer, "  {value}{:padding$}  {count:>count_width$}", "")?;
        }
    }
    Ok(())
}

/// Parses a page's lines through a [`LogBuffer`] exactly as the viewer does.
fn parse_log_page<R: BufRead>(reader: R, source_config: SourceConfig) -> io::Result<LogBuffer> {
    let mut buffer = LogBuffer::unbounded_with_source_config(source_config);
    for line in reader.lines() {
        buffer.push_line(line?);
    }
    let _ = buffer.finish_input();
    Ok(buffer)
}

fn source_counts<R: BufRead>(
    reader: R,
    source_config: SourceConfig,
) -> io::Result<BTreeMap<String, usize>> {
    let buffer = parse_log_page(reader, source_config)?;
    let mut counts = BTreeMap::new();
    for event in buffer.events() {
        *counts.entry(event.source.clone()).or_default() += 1;
    }
    Ok(counts)
}

/// Appends a session's lines to its page log on disk.
///
/// The page log is stored as two segments: the current segment `<id>.log` and
/// the previous, rotated segment `<id>.log.1`. Once the current segment holds
/// `max_lines` lines it is renamed over the rotated segment and a fresh current
/// segment is started, so readers that concatenate both segments always see
/// between `max_lines` and `2 × max_lines` of the most recent lines. Rotation is
/// a rename plus a reopen, so it never reads the log back on the UI thread.
pub(crate) struct PageLogRecorder {
    path: PathBuf,
    rotated_path: PathBuf,
    writer: BufWriter<File>,
    max_lines: usize,
    lines_written: usize,
}

#[derive(Debug)]
pub(crate) struct ActiveLogPageRegistration {
    metadata_path: PathBuf,
    log_path: PathBuf,
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
        let rotated_path = rotated_log_page_path(&path);
        // A rotated segment left by an earlier session with the same id would
        // otherwise be read as this session's history.
        remove_file_if_exists(&rotated_path).map_err(|source| LogPageError::Io {
            action: "remove stale log page segment",
            path: rotated_path.clone(),
            source,
        })?;
        let file = File::create(&path).map_err(|source| LogPageError::Io {
            action: "create log page",
            path: path.clone(),
            source,
        })?;

        Ok(Self {
            path,
            rotated_path,
            writer: BufWriter::new(file),
            max_lines,
            lines_written: 0,
        })
    }

    pub(crate) fn record_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.writer, "{line}")?;
        self.lines_written += 1;
        // Keep the on-disk log bounded to the same window as the in-memory
        // buffer: once the current segment is full, rotate it out so at most
        // two segments' worth of lines are retained.
        if self.max_lines > 0 && self.lines_written >= self.max_lines {
            self.rotate()?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Flush before renaming so the rotated segment ends on a whole line.
        self.writer.flush()?;
        fs::rename(&self.path, &self.rotated_path)?;
        self.writer = BufWriter::new(File::create(&self.path)?);
        self.lines_written = 0;
        Ok(())
    }
}

impl ActiveLogPageRegistration {
    fn new(metadata_path: PathBuf, log_path: PathBuf) -> Self {
        Self {
            metadata_path,
            log_path,
        }
    }
}

impl Drop for ActiveLogPageRegistration {
    fn drop(&mut self) {
        // Remove the log before releasing the id, so a session that claims the
        // id afterwards cannot have its fresh log deleted by us.
        remove_log_page_segments(&self.log_path);
        let _ = fs::remove_file(&self.metadata_path);
    }
}

pub(crate) fn claim_active_log_page(
    requested: Option<LogPageId>,
    command: impl Into<String>,
) -> Result<(LogPageId, ActiveLogPageRegistration), LogPageError> {
    claim_active_log_page_in_dir(
        requested,
        command,
        &log_page_registry_dir(),
        &log_page_dir(),
    )
}

fn claim_active_log_page_in_dir(
    requested: Option<LogPageId>,
    command: impl Into<String>,
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<(LogPageId, ActiveLogPageRegistration), LogPageError> {
    let command = command.into();
    // Reaps metadata left behind by dead processes so their ids can be reused.
    let active = active_log_pages_from_dir(registry_dir, page_dir)?;

    if let Some(requested) = requested {
        let registration =
            try_register_active_log_page(&requested, &command, registry_dir, page_dir)?;
        return Ok((requested, registration));
    }

    for candidate in 1u64.. {
        let id = LogPageId(candidate.to_string());
        if active.iter().any(|page| page.id == id.as_str()) {
            continue;
        }
        match try_register_active_log_page(&id, &command, registry_dir, page_dir) {
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
    registry_dir: &Path,
    page_dir: &Path,
) -> Result<ActiveLogPageRegistration, LogPageError> {
    fs::create_dir_all(registry_dir).map_err(|source| LogPageError::Io {
        action: "create active page directory",
        path: registry_dir.to_path_buf(),
        source,
    })?;

    let path = active_log_page_path_in_dir(id, registry_dir);
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

    Ok(ActiveLogPageRegistration::new(
        path,
        log_page_path_in_dir(id, page_dir),
    ))
}

/// Opens a page's retained window — the rotated segment followed by the
/// current one — as a single stream of lines in recording order.
fn open_log_page(id: &LogPageId, path: &Path) -> Result<Box<dyn Read>, LogPageError> {
    // The recorder may rotate between our two opens, which would skip or
    // duplicate a segment. Rotation always replaces the rotated segment, so
    // detect it by checking that path still names the file we opened.
    const OPEN_ATTEMPTS: usize = 3;

    let rotated_path = rotated_log_page_path(path);
    let mut attempt = 1;
    loop {
        let rotated = open_log_page_segment(&rotated_path)?;
        let current = open_log_page_segment(path)?;
        let unchanged = segment_unchanged(rotated.as_ref(), &rotated_path).map_err(|source| {
            LogPageError::Io {
                action: "open log page",
                path: rotated_path.clone(),
                source,
            }
        })?;
        if !unchanged && attempt < OPEN_ATTEMPTS {
            attempt += 1;
            continue;
        }

        return match (rotated, current) {
            (Some(rotated), Some(current)) => Ok(Box::new(rotated.chain(current))),
            (Some(segment), None) | (None, Some(segment)) => Ok(Box::new(segment)),
            (None, None) => Err(LogPageError::MissingPage {
                id: id.clone(),
                path: path.to_path_buf(),
            }),
        };
    }
}

fn open_log_page_segment(path: &Path) -> Result<Option<File>, LogPageError> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LogPageError::Io {
            action: "open log page",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Whether `path` still names the segment that was opened (or is still absent).
fn segment_unchanged(opened: Option<&File>, path: &Path) -> io::Result<bool> {
    let opened = opened
        .map(|file| file.metadata().map(|metadata| file_identity(&metadata)))
        .transpose()?;
    Ok(opened == path_identity(path)?)
}

/// Identifies the file a path currently names, or `None` when it is missing.
fn path_identity(path: &Path) -> io::Result<Option<FileIdentity>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(file_identity(&metadata))),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(source),
    }
}

#[cfg(unix)]
type FileIdentity = (u64, u64);

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    (metadata.dev(), metadata.ino())
}

// Without a portable inode, only a segment appearing or disappearing is
// detected; the window is still read in order, just possibly stale.
#[cfg(not(unix))]
type FileIdentity = ();

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> FileIdentity {}

fn print_log_page_tail_from_path<W: Write>(
    id: &LogPageId,
    path: &Path,
    options: &LogPageTailOptions,
    writer: &mut W,
) -> Result<(), LogPageError> {
    let file = open_log_page(id, path)?;
    let reader = BufReader::new(file);
    if options.format == LogOutputFormat::Json {
        for record in tail_matching_records(reader, options, path)? {
            write_json_line(
                writer,
                &LogPageRecord::new(&record.event, &record.lines, options.clean),
            )?;
        }
        return Ok(());
    }

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
        let line = if options.clean {
            clean_display_text(&line)
        } else {
            line
        };
        writeln!(writer, "{line}").map_err(LogPageError::Output)?;
    }

    Ok(())
}

/// Returns the raw lines of the last `options.line_count` records matching the
/// filters, each emitted whole (header plus any folded multi-line block).
fn filtered_tail_lines<R: BufRead>(
    reader: R,
    options: &LogPageTailOptions,
    path: &Path,
) -> Result<Vec<String>, LogPageError> {
    Ok(tail_matching_records(reader, options, path)?
        .into_iter()
        .flat_map(|record| record.lines)
        .collect())
}

/// A parsed event together with the raw page-log lines that compose it.
struct MatchedRecord {
    event: LogEvent,
    lines: Vec<String>,
}

/// Returns the last `options.line_count` records matching the filters, in
/// recording order.
///
/// This loads the entire retained window into memory to parse it into events.
/// That window is bounded by the recorder's rotation to at most
/// `2 × buffer_lines` lines, which is the documented bound.
fn tail_matching_records<R: BufRead>(
    reader: R,
    options: &LogPageTailOptions,
    path: &Path,
) -> Result<Vec<MatchedRecord>, LogPageError> {
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
        let removed_group_lines =
            take_removed_group_lines(&groups, &mut group_of_sequence, &change.removed);
        if let Some((&last, replayed)) = change.appended.split_last() {
            // An abandoned fold replays lines that were filed as continuations
            // of the current group; move each back out into its own record.
            regroup_replayed_lines(&mut groups, &mut group_of_sequence, current_group, replayed);
            let index = groups.len();
            let mut group = removed_group_lines;
            group.push(line);
            groups.push(group);
            group_of_sequence.insert(last, index);
            current_group = Some(index);
        } else if let Some(index) = current_group {
            groups[index].push(line);
        }
    }
    let change = buffer.finish_input();
    regroup_replayed_lines(
        &mut groups,
        &mut group_of_sequence,
        current_group,
        &change.appended,
    );

    let filter = log_filter_for_options(options)?;
    let matching = buffer
        .events()
        .iter()
        .filter(|event| filter.matches(event))
        .collect::<Vec<_>>();

    let start = matching.len().saturating_sub(options.line_count);
    let mut records = Vec::new();
    for event in &matching[start..] {
        if let Some(&index) = group_of_sequence.get(&event.sequence) {
            records.push(MatchedRecord {
                event: (*event).clone(),
                lines: std::mem::take(&mut groups[index]),
            });
        }
    }

    Ok(records)
}

/// Gives each replayed event its own record group. The replayed lines are the
/// most recent continuation lines of `current_group`, in order, because every
/// line a fold buffered was filed there while the fold was open.
fn regroup_replayed_lines(
    groups: &mut Vec<Vec<String>>,
    group_of_sequence: &mut HashMap<u64, usize>,
    current_group: Option<usize>,
    replayed: &[u64],
) {
    let Some(current) = current_group else {
        return;
    };
    if replayed.is_empty() {
        return;
    }

    let keep = groups[current].len().saturating_sub(replayed.len());
    let lines = groups[current].split_off(keep);
    for (sequence, line) in replayed.iter().zip(lines) {
        let index = groups.len();
        groups.push(vec![line]);
        group_of_sequence.insert(*sequence, index);
    }
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

fn log_filter_for_options(options: &LogPageTailOptions) -> Result<LogFilter, LogPageError> {
    build_log_filter(
        options.source.as_deref(),
        options.text.as_deref(),
        options.level,
        &options.property_filters,
    )
}

/// The narrowing filter shared by `loggle log` and `loggle facets`.
fn build_log_filter(
    source: Option<&str>,
    text: Option<&str>,
    level: Option<LogLevel>,
    property_filters: &[String],
) -> Result<LogFilter, LogPageError> {
    let mut filter = LogFilter::default();
    filter.source = source
        .map(|source| source.trim().to_string())
        .filter(|source| !source.is_empty());
    filter.text = text
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    filter.level = level;

    for property_filter in property_filters {
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

/// Parses `value` as a JSON number only when re-serializing it reproduces the
/// exact text, so `01`, `1.` or out-of-range integers stay strings.
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

fn log_page_path(id: &LogPageId) -> PathBuf {
    log_page_path_in_dir(id, &log_page_dir())
}

fn log_page_path_in_dir(id: &LogPageId, dir: &Path) -> PathBuf {
    dir.join(format!("{}.log", id.as_str()))
}

/// The previous segment of a page log: `<id>.log.1` next to `<id>.log`.
fn rotated_log_page_path(path: &Path) -> PathBuf {
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    PathBuf::from(rotated)
}

/// Best-effort removal of both segments of a page log.
fn remove_log_page_segments(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(rotated_log_page_path(path));
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(source) if source.kind() != io::ErrorKind::NotFound => Err(source),
        _ => Ok(()),
    }
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

fn active_log_pages_from_dir(
    dir: &Path,
    page_dir: &Path,
) -> Result<Vec<ActiveLogPage>, LogPageError> {
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
            // The owning process is gone: drop both log segments and then its
            // metadata so stale pages do not accumulate in the state directory.
            if let Ok(id) = LogPageId::parse(&page.id) {
                remove_log_page_segments(&log_page_path_in_dir(&id, page_dir));
            }
            let _ = fs::remove_file(path);
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
    fn clean_tail_preserves_whole_records_and_does_not_change_storage_or_matching() {
        let path = env::temp_dir().join(format!("loggle-clean-test-{}.log", std::process::id()));
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        let colored = fixture
            .lines()
            .map(|line| format!("\x1b[31m{line}\x1b[0m\x1b]0;title\x07\n"))
            .collect::<String>();
        fs::write(&path, &colored).unwrap();
        let id = LogPageId::parse("clean-test").unwrap();
        let mut options = LogPageTailOptions::new(1);
        options
            .property_filters
            .push("requestId=fixture-failed".to_string());
        for filtered in [true, false] {
            if !filtered {
                options.property_filters.clear();
            }
            options.clean = false;
            let mut raw = Vec::new();
            print_log_page_tail_from_path(&id, &path, &options, &mut raw).unwrap();
            assert!(raw.contains(&0x1b));
            options.clean = true;
            let mut clean = Vec::new();
            print_log_page_tail_from_path(&id, &path, &options, &mut clean).unwrap();
            let expected = String::from_utf8(raw)
                .unwrap()
                .lines()
                .map(|line| format!("{}\n", clean_display_text(line)))
                .collect::<String>();
            assert_eq!(String::from_utf8(clean.clone()).unwrap(), expected);
            assert!(!clean.contains(&0x1b));
            assert_eq!(expected.lines().count(), if filtered { 7 } else { 1 });
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), colored);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn source_discovery_uses_event_counts_and_source_promotion() {
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        let counts =
            source_counts(BufReader::new(fixture.as_bytes()), SourceConfig::default()).unwrap();
        let app = {
            let mut app = crate::app::App::with_source_config(100, SourceConfig::default());
            for line in fixture.lines() {
                app.push_line(line.to_string());
            }
            app
        };
        assert_eq!(
            counts,
            app.source_status_rows()
                .into_iter()
                .map(|row| (row.source, row.count))
                .collect()
        );
        assert_eq!(counts.values().sum::<usize>(), 12);
        assert_eq!(counts["api"], 5);
        let input = "{\"message\":\"ready\",\"unit\":\"custom\"}\nplain\n#7 CACHED\n";
        let counts = source_counts(
            BufReader::new(input.as_bytes()),
            SourceConfig::with_fields(["unit"]),
        )
        .unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("build".into(), 1),
                ("custom".into(), 1),
                ("unknown".into(), 1)
            ])
        );
        assert!(
            source_counts(BufReader::new(&b""[..]), SourceConfig::default())
                .unwrap()
                .is_empty()
        );
        assert!(source_counts(BufReader::new(&b"\xff"[..]), SourceConfig::default()).is_err());
    }

    fn fixture_facets(options: &LogPageFacetOptions) -> Vec<FacetGroup> {
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        facet_groups(BufReader::new(fixture.as_bytes()), options, Path::new("t")).unwrap()
    }

    fn bucket_counts(group: &FacetGroup) -> Vec<(&str, usize)> {
        group
            .buckets
            .iter()
            .map(|bucket| (bucket.value.as_str(), bucket.count))
            .collect()
    }

    #[test]
    fn facets_default_to_source_level_and_property_key_over_the_fixture() {
        let groups = fixture_facets(&LogPageFacetOptions::default());

        assert_eq!(
            groups.iter().map(|group| group.facet).collect::<Vec<_>>(),
            [FacetKind::Source, FacetKind::Level, FacetKind::PropertyKey]
        );
        assert_eq!(
            bucket_counts(&groups[0]),
            [
                ("api", 5),
                ("worker", 4),
                ("database", 2),
                ("minio-init", 1)
            ]
        );
        assert_eq!(groups[0].available_records, 12);
        assert_eq!(groups[0].eligible_records, 12);
        assert_eq!(bucket_counts(&groups[1]), [("error", 3), ("info", 9)]);
        assert!(
            bucket_counts(&groups[2]).contains(&("requestId", 11)),
            "{:?}",
            groups[2]
        );
    }

    #[test]
    fn facets_count_property_values_and_honour_selection_and_filters() {
        let mut options = LogPageFacetOptions {
            property_key: Some(" requestId ".to_string()),
            facets: vec![FacetKind::Source],
            ..LogPageFacetOptions::default()
        };
        let groups = fixture_facets(&options);
        assert_eq!(
            groups.iter().map(|group| group.facet).collect::<Vec<_>>(),
            [FacetKind::Source, FacetKind::PropertyValue]
        );
        let values = &groups[1];
        assert_eq!(values.property_key.as_deref(), Some("requestId"));
        assert_eq!(
            bucket_counts(values),
            [
                ("fixture-failed", 5),
                ("fixture-success", 5),
                ("fixture-failed-extra", 1)
            ]
        );

        // `--level error` narrows every other facet but not the level facet.
        options.facets = vec![FacetKind::Level, FacetKind::Source];
        options.level = Some(LogLevel::Error);
        let groups = fixture_facets(&options);
        assert_eq!(
            bucket_counts(&groups[0]),
            [("api", 1), ("database", 1), ("worker", 1)]
        );
        assert_eq!(groups[0].matched_records, 3);
        assert_eq!(bucket_counts(&groups[1]), [("error", 3), ("info", 9)]);
        assert_eq!(bucket_counts(&groups[2]), [("fixture-failed", 3)]);

        options.level = None;
        options.source = Some("worker".to_string());
        options.text = Some("job".to_string());
        options.property_filters = vec!["jobId=job-002".to_string()];
        let groups = fixture_facets(&options);
        assert_eq!(bucket_counts(&groups[2]), [("fixture-success", 2)]);

        options.property_filters = vec!["=".to_string()];
        assert!(matches!(
            facet_groups(BufReader::new(&b""[..]), &options, Path::new("t")),
            Err(LogPageError::InvalidPropertyFilter(_))
        ));
        options.property_filters.clear();
        options.property_key = Some("  ".to_string());
        assert!(matches!(
            facet_groups(BufReader::new(&b""[..]), &options, Path::new("t")),
            Err(LogPageError::InvalidFacetOptions(_))
        ));
    }

    #[test]
    fn facets_bound_the_record_window_and_bucket_count() {
        let options = LogPageFacetOptions {
            record_limit: 5,
            bucket_limit: 1,
            facets: vec![FacetKind::Source],
            ..LogPageFacetOptions::default()
        };
        let groups = fixture_facets(&options);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].window_records, 5);
        assert!(groups[0].window_truncated);
        assert_eq!(groups[0].total_buckets, 3);
        assert!(groups[0].truncated);
        assert_eq!(bucket_counts(&groups[0]), [("api", 2)]);

        let clamped = fixture_facets(&LogPageFacetOptions {
            record_limit: 0,
            bucket_limit: 0,
            ..options
        });
        assert_eq!(clamped[0].window_records, MIN_FACET_RECORD_LIMIT);
        assert_eq!(clamped[0].buckets.len(), MIN_FACET_BUCKET_LIMIT);
    }

    #[test]
    fn facet_json_lines_match_the_schema_version_1_shape() {
        let (root, _, page_dir) = test_state_dir("facet-json-test");
        let id = LogPageId::parse("facets").unwrap();
        let path = log_page_path_in_dir(&id, &page_dir);
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(
            &path,
            include_str!("../fixtures/mixed-service-investigation.log"),
        )
        .unwrap();
        let options = LogPageFacetOptions {
            property_key: Some("requestId".to_string()),
            property_filters: vec!["requestId=fixture-failed".to_string()],
            format: LogOutputFormat::Json,
            ..LogPageFacetOptions::default()
        };
        let mut output = Vec::new();
        print_log_page_facets_from_path(&id, &path, &options, &mut output).unwrap();
        let lines = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            lines
                .iter()
                .map(|line| line["facet"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["source", "level", "property_key", "property_value"]
        );
        assert!(lines.iter().all(|line| line["schema_version"] == 1));
        assert_eq!(
            lines[3],
            serde_json::json!({
                "schema_version": 1,
                "facet": "property_value",
                "property_key": "requestId",
                "available_records": 12,
                "window_records": 12,
                "window_truncated": false,
                "matched_records": 5,
                "eligible_records": 12,
                "total_buckets": 3,
                "truncated": false,
                "buckets": [
                    {"value": "fixture-failed", "count": 5, "value_types": ["string", "text"]},
                    {"value": "fixture-success", "count": 5, "value_types": ["string", "text"]},
                    {"value": "fixture-failed-extra", "count": 1, "value_types": ["text"]},
                ],
            })
        );
        assert_eq!(lines[0]["property_key"], serde_json::Value::Null);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facet_text_output_is_aligned_and_escaped() {
        let input = "api | INFO a key=x\napi | INFO b\nweb | ERROR c\n";
        let options = LogPageFacetOptions {
            facets: vec![FacetKind::Source, FacetKind::Level],
            bucket_limit: 1,
            ..LogPageFacetOptions::default()
        };
        let groups =
            facet_groups(BufReader::new(input.as_bytes()), &options, Path::new("t")).unwrap();
        let mut output = Vec::new();
        write_facet_text(&mut output, &groups).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "source (3 records, 2 buckets, top 1 shown)\n  api  2\n\nlevel (3 records, 2 buckets, top 1 shown)\n  error  1\n"
        );

        let mut group = groups[0].clone();
        group.property_key = Some("multi\nline".to_string());
        group.buckets[0].value = "two\nlines".to_string();
        group.buckets.push(group.buckets[0].clone());
        group.buckets[1].value = "x".to_string();
        group.buckets[1].count = 12;
        group.buckets.push(group.buckets[1].clone());
        group.buckets[2].value = String::new();
        group.total_buckets = 3;
        group.truncated = false;
        let mut output = Vec::new();
        write_facet_text(&mut output, &[group]).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "source multi\\nline (3 records, 3 buckets)\n  two\\nlines   2\n  x           12\n              12\n"
        );

        let empty = facet_groups(BufReader::new(&b""[..]), &options, Path::new("t")).unwrap();
        let mut output = Vec::new();
        write_facet_text(&mut output, &empty).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "source (0 records, 0 buckets)\n  (none)\n\nlevel (0 records, 0 buckets)\n  (none)\n"
        );
    }

    #[test]
    fn facets_report_a_missing_page() {
        let (root, _, page_dir) = test_state_dir("facet-missing-test");
        let id = LogPageId::parse("missing").unwrap();
        let path = log_page_path_in_dir(&id, &page_dir);

        let error = print_log_page_facets_from_path(
            &id,
            &path,
            &LogPageFacetOptions::default(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(matches!(error, LogPageError::MissingPage { .. }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn filtered_tail_lines_matches_source() {
        let input = "api | one\nweb | two\napi | three\n".as_bytes();
        let options = LogPageTailOptions {
            line_count: 2,
            clean: false,
            source: Some("api".to_string()),
            text: None,
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
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
            clean: false,
            source: Some("api".to_string()),
            text: None,
            property_filters: vec!["tenantId=tenant-1".to_string()],
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
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
            clean: false,
            source: None,
            text: None,
            property_filters: vec!["tenantId=tenant-1".to_string()],
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
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
            clean: false,
            source: None,
            text: None,
            property_filters: vec!["tenantId=t1".to_string()],
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
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
            clean: false,
            source: Some("api".to_string()),
            text: Some("database".to_string()),
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
        };

        let lines =
            filtered_tail_lines(BufReader::new(input), &options, Path::new("test.log")).unwrap();

        assert_eq!(lines, vec!["api | ERROR database unavailable".to_string()]);
    }

    /// Records as `loggle log --json` would emit them, parsed back as JSON.
    fn json_records(input: &str, options: &LogPageTailOptions) -> Vec<serde_json::Value> {
        tail_matching_records(
            BufReader::new(input.as_bytes()),
            options,
            Path::new("test.log"),
        )
        .unwrap()
        .iter()
        .map(|record| {
            let mut line = Vec::new();
            write_json_line(
                &mut line,
                &LogPageRecord::new(&record.event, &record.lines, options.clean),
            )
            .unwrap();
            let line = String::from_utf8(line).unwrap();
            assert_eq!(line.lines().count(), 1, "one record per line: {line}");
            serde_json::from_str(&line).unwrap()
        })
        .collect()
    }

    #[test]
    fn json_record_types_properties_with_lossless_numbers() {
        let input = "api | INFO values canonical=500 negative=-3 float=1.5 leading=01 trailing=1. huge=18446744073709551616 flag=true off=false missing=null label=\"ok\" text=bare dup=first dup=second\n";
        let records = json_records(input, &LogPageTailOptions::new(10));

        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record["schema_version"], 1);
        assert_eq!(record["sequence"], 0);
        assert_eq!(record["source"], "api");
        assert_eq!(record["timestamp"], serde_json::Value::Null);
        assert_eq!(record["level"], "info");
        assert!(record["message"].as_str().unwrap().starts_with("values"));
        let properties = &record["properties"];
        assert_eq!(properties["canonical"], serde_json::json!(500));
        assert_eq!(properties["negative"], serde_json::json!(-3));
        assert_eq!(properties["float"], serde_json::json!(1.5));
        assert_eq!(properties["leading"], "01");
        assert_eq!(properties["trailing"], "1.");
        assert_eq!(properties["huge"], "18446744073709551616");
        assert_eq!(properties["flag"], true);
        assert_eq!(properties["off"], false);
        assert_eq!(properties["missing"], serde_json::Value::Null);
        assert_eq!(properties["label"], "ok");
        assert_eq!(properties["text"], "bare");
        assert_eq!(properties["dup"], "first");
        assert_eq!(record["raw"], input.trim_end());
    }

    #[test]
    fn json_record_matches_the_schema_version_1_shape_with_multiline_raw() {
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        let mut options = LogPageTailOptions::new(1);
        options.property_filters = vec!["requestId=fixture-failed".to_string()];
        let records = json_records(fixture, &options);

        assert_eq!(records.len(), 1);
        let raw = fixture.lines().collect::<Vec<_>>()[6..13].join("\n");
        assert_eq!(
            records[0],
            serde_json::json!({
                "schema_version": 1,
                "sequence": records[0]["sequence"],
                "source": "api",
                "timestamp": "10:00:00.050",
                "level": "error",
                "message": "request failed",
                "properties": {
                    "cause": "job insert rejected: missing synthetic parent",
                    "requestId": "fixture-failed",
                    "statusCode": 500,
                },
                "raw": raw,
            })
        );
        assert!(records[0]["sequence"].is_u64());
        assert_eq!(records[0]["raw"].as_str().unwrap().lines().count(), 7);
    }

    #[test]
    fn json_without_filters_emits_one_record_per_event_with_increasing_sequence() {
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        let records = json_records(fixture, &LogPageTailOptions::new(100));
        let sequences = records
            .iter()
            .map(|record| record["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(records.len(), 12);
        assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
        let raw_lines = records
            .iter()
            .map(|record| record["raw"].as_str().unwrap().lines().count())
            .sum::<usize>();
        assert_eq!(raw_lines, fixture.lines().count());
    }

    #[test]
    fn level_filter_composes_with_source_text_and_property_filters() {
        let input = "api | ERROR database failed tenantId=tenant-1\napi | ERROR database failed tenantId=tenant-2\napi | INFO database failed tenantId=tenant-1\nweb | ERROR database failed tenantId=tenant-1\napi | WARN database slow tenantId=tenant-1\n";
        let mut options = LogPageTailOptions::new(10);
        options.level = LogLevel::parse("ERR");
        assert_eq!(
            log_filter_for_options(&options).unwrap().level,
            options.level
        );

        let lines = |options: &LogPageTailOptions| {
            filtered_tail_lines(BufReader::new(input.as_bytes()), options, Path::new("t")).unwrap()
        };
        assert_eq!(lines(&options).len(), 3);
        options.source = Some("api".to_string());
        options.text = Some("database".to_string());
        options.property_filters = vec!["tenantId=tenant-1".to_string()];
        assert_eq!(
            lines(&options),
            ["api | ERROR database failed tenantId=tenant-1"]
        );
        options.level = Some(LogLevel::Warn);
        assert_eq!(
            lines(&options),
            ["api | WARN database slow tenantId=tenant-1"]
        );
        options.level = Some(LogLevel::Fatal);
        assert!(lines(&options).is_empty());
        options.format = LogOutputFormat::Json;
        assert!(json_records(input, &options).is_empty());
    }

    #[test]
    fn level_filter_alone_takes_the_record_path_in_text_output() {
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        let raw = fixture.lines().collect::<Vec<_>>();
        let mut options = LogPageTailOptions::new(100);
        options.level = Some(LogLevel::Error);

        let lines =
            filtered_tail_lines(BufReader::new(fixture.as_bytes()), &options, Path::new("t"))
                .unwrap();

        assert_eq!(lines, [&raw[4..5], &raw[5..13]].concat());
        options.format = LogOutputFormat::Json;
        let records = json_records(fixture, &options);
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|record| record["level"] == "error"));
    }

    #[test]
    fn json_output_with_clean_strips_raw_only_and_empty_matches_emit_nothing() {
        let (root, _, page_dir) = test_state_dir("json-clean-test");
        let id = LogPageId::parse("json").unwrap();
        let path = log_page_path_in_dir(&id, &page_dir);
        fs::create_dir_all(&page_dir).unwrap();
        let colored = "\x1b[31mapi | ERROR boom tenantId=t1\x1b[0m\n[api] INFO ok\n";
        fs::write(&path, colored).unwrap();
        let mut options = LogPageTailOptions::new(10);
        options.format = LogOutputFormat::Json;
        options.property_filters = vec!["tenantId=t1".to_string()];
        let query = |options: &LogPageTailOptions| {
            let mut output = Vec::new();
            print_log_page_tail_from_path(&id, &path, options, &mut output).unwrap();
            String::from_utf8(output).unwrap()
        };

        let raw: serde_json::Value = serde_json::from_str(query(&options).trim_end()).unwrap();
        assert!(raw["raw"].as_str().unwrap().contains('\x1b'));
        options.clean = true;
        let clean: serde_json::Value = serde_json::from_str(query(&options).trim_end()).unwrap();
        assert_eq!(clean["raw"], "api | ERROR boom tenantId=t1");
        assert_eq!(clean["message"], raw["message"]);
        assert_eq!(clean["properties"], raw["properties"]);

        options.property_filters = vec!["tenantId=missing".to_string()];
        assert_eq!(query(&options), "");
        assert_eq!(fs::read_to_string(&path).unwrap(), colored);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_page_json_record_is_versioned_and_flat() {
        let page = ActiveLogPage {
            id: "api".to_string(),
            pid: 42,
            started_unix_seconds: 7,
            command: "docker compose up".to_string(),
        };
        let mut output = Vec::new();
        write_json_line(&mut output, &ActiveLogPageRecord::new(&page)).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "{\"schema_version\":1,\"id\":\"api\",\"pid\":42,\"started_unix_seconds\":7,\"command\":\"docker compose up\"}\n"
        );
    }

    #[test]
    fn filtered_tail_lines_includes_property_block_that_precedes_summary() {
        let input = "[api] [21:05:37.312] INFO (#140):\n[api] {\n[api] requestId: \"abc-123\",\n[api] statusCode: 200,\n[api] }\n[api] 21:05:37.312 INFO http.request ok\n"
            .as_bytes();
        let options = LogPageTailOptions {
            line_count: 5,
            clean: false,
            source: None,
            text: None,
            property_filters: vec!["requestId=abc-123".to_string()],
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
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
    fn abandoned_fold_lines_become_their_own_records() {
        // The fold is interrupted by a new summary and again by EOF; every
        // buffered line must come back as a record of its own rather than be
        // folded into the preceding summary or lost.
        let input = "[api] 10:00:00.000 INFO first\n[api] [10:00:00.000] INFO (#1):\n[api] {\n[api] partial: true,\n[api] 10:00:01.000 INFO second\n[api] [10:00:01.000] INFO (#2):\n[api] {\n[api] tail: 1,\n"
            .as_bytes();
        let options = LogPageTailOptions {
            line_count: 100,
            clean: false,
            source: None,
            text: Some("partial".to_string()),
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
        };
        let replay = |options: &LogPageTailOptions| {
            filtered_tail_lines(BufReader::new(input), options, Path::new("test.log")).unwrap()
        };

        assert_eq!(replay(&options), vec!["[api] partial: true,".to_string()]);

        let mut options = options;
        options.text = Some("tail".to_string());
        assert_eq!(replay(&options), vec!["[api] tail: 1,".to_string()]);

        options.text = Some("second".to_string());
        assert_eq!(
            replay(&options),
            vec!["[api] 10:00:01.000 INFO second".to_string()]
        );
    }

    #[test]
    fn mixed_service_fixture_replays_exact_correlations_and_whole_records() {
        let fixture = include_str!("../fixtures/mixed-service-investigation.log");
        let raw = fixture.lines().collect::<Vec<_>>();
        let mut options = LogPageTailOptions {
            line_count: 100,
            clean: false,
            source: None,
            text: None,
            property_filters: vec!["requestId=fixture-failed".to_string()],
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
        };
        let replay = |options: &LogPageTailOptions| {
            filtered_tail_lines(
                BufReader::new(fixture.as_bytes()),
                options,
                Path::new("fixtures/mixed-service-investigation.log"),
            )
            .unwrap()
        };

        // Five failed-request records, including the entire seven-line API error.
        // The interleaved prefix-collision ID and the later success are excluded.
        assert_eq!(replay(&options), [&raw[1..3], &raw[4..13]].concat());
        options.line_count = 1;
        assert_eq!(replay(&options), raw[6..13]);
        options.line_count = 100;
        options.source = Some("database".to_string());
        options.text = Some("job insert rejected".to_string());
        assert_eq!(replay(&options), raw[4..5]);
        options.source = None;
        options.text = None;
        options.property_filters = vec!["requestId=fixture-success".to_string()];
        assert_eq!(replay(&options), raw[13..18]);
        options.property_filters = vec!["requestId=fixture-failed-extra".to_string()];
        assert_eq!(replay(&options), raw[3..4]);
        options.property_filters = vec!["requestId=fixture".to_string()];
        assert!(replay(&options).is_empty());
        options.property_filters.clear();
        options.source = Some("minio-init".to_string());
        assert_eq!(replay(&options), raw[0..1]);
    }

    #[test]
    fn vev_compose_fixture_replays_buildkit_and_status_filters() {
        let options = LogPageTailOptions {
            line_count: 10,
            clean: false,
            source: Some("vev-statistics".to_string()),
            text: Some("npm ci".to_string()),
            property_filters: Vec::new(),
            source_config: SourceConfig::default(),
            level: None,
            format: LogOutputFormat::Text,
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

    /// A fresh state directory for one test, with separate registry and page
    /// subdirectories so tests never touch the user's real page logs.
    fn test_state_dir(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = env::temp_dir().join(format!("loggle-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let registry_dir = root.join("active-pages");
        let page_dir = root.join("pages");
        (root, registry_dir, page_dir)
    }

    fn read_page(id: &LogPageId, path: &Path, line_count: usize) -> Vec<String> {
        let mut output = Vec::new();
        print_log_page_tail_from_path(id, path, &LogPageTailOptions::new(line_count), &mut output)
            .unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn page_log_recorder_truncates_and_flushes_lines() {
        let (root, _, page_dir) = test_state_dir("page-recorder-test");
        let path = page_dir.join("recorder.log");
        let rotated_path = rotated_log_page_path(&path);
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(&path, "old\n").unwrap();
        fs::write(&rotated_path, "stale\n").unwrap();

        {
            let mut recorder = PageLogRecorder::create_at_path(path.clone(), 100).unwrap();
            // A previous session's segments must not leak into this page.
            assert!(!rotated_path.exists());
            recorder.record_line("one").unwrap();
            recorder.record_line("two").unwrap();
            recorder.flush().unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
            assert!(!rotated_path.exists());
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn page_log_recorder_compacts_to_stay_within_bound() {
        let (root, _, page_dir) = test_state_dir("page-recorder-compact-test");
        let path = page_dir.join("recorder.log");
        let rotated_path = rotated_log_page_path(&path);

        {
            let mut recorder = PageLogRecorder::create_at_path(path.clone(), 2).unwrap();
            for index in 0..7 {
                recorder.record_line(&format!("line {index}")).unwrap();
                recorder.flush().unwrap();
                let current = fs::read_to_string(&path).unwrap().lines().count();
                let rotated = fs::read_to_string(&rotated_path)
                    .map(|contents| contents.lines().count())
                    .unwrap_or(0);
                // Each segment holds less than a full window, so together they
                // never exceed twice the cap.
                assert!(current < 2, "current segment over cap after {index}");
                assert!(rotated <= 2, "rotated segment over cap after {index}");
            }
        }

        // Rotation is a rename: the full previous segment moves aside intact
        // and the current segment keeps only what was written since.
        assert_eq!(
            fs::read_to_string(&rotated_path).unwrap(),
            "line 4\nline 5\n"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "line 6\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn page_log_reader_sees_lines_across_rotation_boundary_in_order() {
        let (root, _, page_dir) = test_state_dir("page-rotation-read-test");
        let id = LogPageId::parse("rotation").unwrap();
        let path = log_page_path_in_dir(&id, &page_dir);

        let mut recorder = PageLogRecorder::create_at_path(path.clone(), 3).unwrap();
        for index in 0..8 {
            recorder
                .record_line(&format!("api | INFO line {index}"))
                .unwrap();
        }
        recorder.flush().unwrap();

        // Rotated segment holds lines 3..6 and the current one holds 6..8.
        let expected = (3..8)
            .map(|index| format!("api | INFO line {index}"))
            .collect::<Vec<_>>();
        assert_eq!(read_page(&id, &path, 100), expected);
        assert_eq!(read_page(&id, &path, 3), expected[2..]);

        let mut options = LogPageTailOptions::new(3);
        options.text = Some("line".to_string());
        let mut output = Vec::new();
        print_log_page_tail_from_path(&id, &path, &options, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            expected[2..]
        );

        // Immediately after a rotation the current segment is empty and the
        // whole window comes from the rotated segment.
        recorder.record_line("api | INFO line 8").unwrap();
        recorder.flush().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert_eq!(read_page(&id, &path, 1), vec!["api | INFO line 8"]);

        drop(recorder);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_page_reports_missing_page_error() {
        let (root, _, page_dir) = test_state_dir("missing-page-test");
        let id = LogPageId::parse("missing").unwrap();
        let path = log_page_path_in_dir(&id, &page_dir);

        let error =
            print_log_page_tail_from_path(&id, &path, &LogPageTailOptions::new(1), &mut Vec::new())
                .unwrap_err();

        assert!(matches!(error, LogPageError::MissingPage { .. }));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn reaping_dead_page_removes_both_log_segments() {
        let (root, registry_dir, page_dir) = test_state_dir("reap-page-test");
        let id = LogPageId::parse("dead").unwrap();
        let log_path = log_page_path_in_dir(&id, &page_dir);
        let rotated_path = rotated_log_page_path(&log_path);
        let metadata_path = active_log_page_path_in_dir(&id, &registry_dir);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(&log_path, "current\n").unwrap();
        fs::write(&rotated_path, "rotated\n").unwrap();
        let dead = ActiveLogPage {
            id: id.as_str().to_string(),
            // Pid 0 is never a live page owner.
            pid: 0,
            started_unix_seconds: 0,
            command: "gone".to_string(),
        };
        fs::write(&metadata_path, serde_json::to_string(&dead).unwrap()).unwrap();

        assert!(
            active_log_pages_from_dir(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );

        assert!(!metadata_path.exists());
        assert!(!log_path.exists());
        assert!(!rotated_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_log_page_registration_writes_and_removes_metadata() {
        let (root, registry_dir, page_dir) = test_state_dir("active-page-test");
        let id = LogPageId::parse("api").unwrap();
        let log_path = log_page_path_in_dir(&id, &page_dir);
        let rotated_path = rotated_log_page_path(&log_path);

        {
            let (claimed, _registration) = claim_active_log_page_in_dir(
                Some(id),
                "docker compose up",
                &registry_dir,
                &page_dir,
            )
            .unwrap();
            assert_eq!(claimed.as_str(), "api");
            let pages = active_log_pages_from_dir(&registry_dir, &page_dir).unwrap();

            assert_eq!(pages.len(), 1);
            assert_eq!(pages[0].id, "api");
            assert_eq!(pages[0].pid, std::process::id());
            assert_eq!(pages[0].command, "docker compose up");

            let mut recorder = PageLogRecorder::create_at_path(log_path.clone(), 1).unwrap();
            recorder.record_line("one").unwrap();
            recorder.record_line("two").unwrap();
            recorder.flush().unwrap();
            assert!(log_path.exists());
            assert!(rotated_path.exists());
        }

        assert!(
            active_log_pages_from_dir(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        assert!(!log_path.exists());
        assert!(!rotated_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_log_pages_ignore_invalid_metadata_files() {
        let (root, registry_dir, page_dir) = test_state_dir("invalid-active-page-test");
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(registry_dir.join("invalid.json"), "{").unwrap();

        assert!(
            active_log_pages_from_dir(&registry_dir, &page_dir)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claim_active_log_page_allocates_first_available_numeric_id() {
        let (root, registry_dir, page_dir) = test_state_dir("allocate-page-id-test");
        let one = LogPageId::parse("1").unwrap();
        let three = LogPageId::parse("3").unwrap();
        let (_, _one) =
            claim_active_log_page_in_dir(Some(one), "one", &registry_dir, &page_dir).unwrap();
        let (_, _three) =
            claim_active_log_page_in_dir(Some(three), "three", &registry_dir, &page_dir).unwrap();

        let (id, _registration) =
            claim_active_log_page_in_dir(None, "two", &registry_dir, &page_dir).unwrap();

        assert_eq!(id.as_str(), "2");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claim_active_log_page_rejects_active_requested_id() {
        let (root, registry_dir, page_dir) = test_state_dir("requested-page-id-test");
        let id = LogPageId::parse("api").unwrap();
        let (_, _registration) =
            claim_active_log_page_in_dir(Some(id.clone()), "api", &registry_dir, &page_dir)
                .unwrap();

        let error =
            claim_active_log_page_in_dir(Some(id), "api", &registry_dir, &page_dir).unwrap_err();

        assert_eq!(error.to_string(), "log page id 'api' is already active");
        let _ = fs::remove_dir_all(root);
    }
}
