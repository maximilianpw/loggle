use std::collections::{HashMap, VecDeque};

use crate::model::{
    LogEvent, LogInterpreter, LogProperty, ParsedLine, PropertyFoldHeader, SourceConfig,
    parse_buildkit_step_line,
};

/// An open `{ ... }` property block holding more lines than this is abandoned.
pub(crate) const MAX_PENDING_PROPERTY_LINES: usize = 256;
/// An open property block whose buffered, source-stripped lines (each counted
/// with one newline byte) would exceed this many bytes is abandoned.
pub(crate) const MAX_PENDING_PROPERTY_BYTES: usize = 256 * 1024;
/// How many of the newest events a sourced property-block header searches for
/// the latest event of its own source. Interleaved output from other sources
/// rarely spans more than a few lines, and the bound keeps a header from
/// scanning a large retained page; past it the header is treated as unmatched.
const MAX_SOURCED_TARGET_LOOKBACK: usize = 256;

#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<LogEvent>,
    pending_properties: Option<PendingPropertyBlock>,
    completed_property_blocks: VecDeque<CompletedPropertyBlock>,
    active_source: Option<String>,
    buildkit_steps: HashMap<String, String>,
    interpreter: LogInterpreter,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BufferChange {
    pub(crate) appended: Option<u64>,
    pub(crate) removed: Vec<u64>,
    pub(crate) updated: Vec<u64>,
}

/// A property block whose `{` has not yet been closed.
#[derive(Debug)]
struct PendingPropertyBlock {
    /// The event the block's properties are applied to: the matching summary
    /// when the header followed it, otherwise the header's own event.
    target_sequence: u64,
    header: PropertyFoldHeader,
    /// The header had no summary yet, so it was kept as its own event and the
    /// properties move to the summary when it arrives.
    deferred_header: bool,
    lines: Vec<String>,
    bytes: usize,
    brace_depth: i32,
    saw_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPushResult {
    /// The line belongs to the open block, which is still open.
    Accepted,
    /// The line closed the block.
    Complete,
    /// The line cannot belong to the block (a new record or header, another
    /// source, or a cap was hit). The block is dropped without applying any
    /// properties and the line is pushed as an ordinary line.
    AbandonAndRetry,
}

#[derive(Debug)]
struct CompletedPropertyBlock {
    header_sequence: u64,
    header: PropertyFoldHeader,
    properties: Vec<LogProperty>,
}

impl LogBuffer {
    #[cfg(test)]
    pub fn new(capacity: usize) -> Self {
        Self::with_source_config(capacity, SourceConfig::default())
    }

    pub fn with_source_config(capacity: usize, source_config: SourceConfig) -> Self {
        Self::build(capacity, VecDeque::with_capacity(capacity), source_config)
    }

    pub(crate) fn unbounded_with_source_config(source_config: SourceConfig) -> Self {
        // `VecDeque::with_capacity(usize::MAX)` would abort, so the unbounded
        // buffer starts empty and grows on demand.
        Self::build(usize::MAX, VecDeque::new(), source_config)
    }

    fn build(capacity: usize, events: VecDeque<LogEvent>, source_config: SourceConfig) -> Self {
        Self {
            capacity,
            next_sequence: 0,
            events,
            pending_properties: None,
            completed_property_blocks: VecDeque::new(),
            active_source: None,
            buildkit_steps: HashMap::new(),
            interpreter: LogInterpreter::new(source_config),
        }
    }

    pub(crate) fn push_line(&mut self, line: String) -> BufferChange {
        let mut change = BufferChange::default();
        match self.push_pending_property_line(&line, &mut change) {
            Some(PendingPushResult::Accepted | PendingPushResult::Complete) => {}
            Some(PendingPushResult::AbandonAndRetry) | None => {
                self.push_ordinary_line(line, &mut change);
            }
        }
        change
    }

    /// Ends input: an unclosed property block is dropped without applying any
    /// of its partial properties. Call once the source reaches EOF.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "EOF callers in runtime/terminal.rs and page_log.rs are wired separately"
        )
    )]
    pub(crate) fn finish_input(&mut self) {
        self.pending_properties = None;
    }

    fn push_ordinary_line(&mut self, line: String, change: &mut BufferChange) {
        let Some(header) = self.interpreter.property_block_header(&line) else {
            self.push_event(line, change);
            return;
        };

        if let Some(target_sequence) = self.property_target_sequence(&header) {
            self.pending_properties =
                Some(PendingPropertyBlock::new(target_sequence, header, false));
            return;
        }

        if let Some(target_sequence) = self.push_event(line, change) {
            self.pending_properties =
                Some(PendingPropertyBlock::new(target_sequence, header, true));
        }
    }

    fn push_event(&mut self, line: String, change: &mut BufferChange) -> Option<u64> {
        let mut parsed = self.interpreter.parse_source_line(&line);
        self.apply_buildkit_source_context(&line, &mut parsed);
        self.apply_source_context(&mut parsed);

        if self.capacity == 0 {
            self.next_sequence += 1;
            return None;
        }

        if self.events.len() == self.capacity
            && let Some(event) = self.events.pop_front()
        {
            change.removed.push(event.sequence);
        }

        let sequence = self.next_sequence;
        let event = self
            .interpreter
            .event_from_source_line(self.next_sequence, line, parsed);
        self.next_sequence += 1;
        self.events.push_back(event);
        change.appended = Some(sequence);
        self.apply_completed_property_block_to_back(change);
        Some(sequence)
    }

    fn apply_buildkit_source_context(&mut self, line: &str, parsed: &mut ParsedLine) {
        // Recognize the anchored BuildKit step before trusting a generic '|'
        // prefix: a RUN instruction can itself contain shell pipelines.
        let Some(buildkit) = parse_buildkit_step_line(line) else {
            return;
        };

        if let Some(source) = buildkit.source {
            self.buildkit_steps
                .insert(buildkit.step_id.clone(), source.clone());
            parsed.source = source;
            parsed.message = buildkit.message;
            parsed.source_explicit = true;
            return;
        }

        if let Some(source) = self.buildkit_steps.get(&buildkit.step_id) {
            parsed.source = source.clone();
        } else {
            // A standalone CACHED/DONE record (or an ambiguous stage header)
            // proves build activity, not a particular Compose service.
            parsed.source = "build".to_string();
        }
        parsed.message = buildkit.message;
        parsed.source_explicit = true;
    }

    fn apply_source_context(&mut self, parsed: &mut ParsedLine) {
        if parsed.source_explicit {
            self.active_source = Some(parsed.source.clone());
            return;
        }

        if is_continuation_line(&parsed.message) {
            if let Some(source) = self.active_source.as_ref() {
                parsed.source = source.clone();
            }
        } else {
            self.active_source = None;
        }
    }

    fn push_pending_property_line(
        &mut self,
        line: &str,
        change: &mut BufferChange,
    ) -> Option<PendingPushResult> {
        let mut pending = self.pending_properties.take()?;

        let buildkit_source = self.pending_buildkit_source(line);
        let result = pending.push_line(&self.interpreter, line, buildkit_source);
        match result {
            PendingPushResult::Accepted => self.pending_properties = Some(pending),
            PendingPushResult::Complete => self.apply_pending_properties(pending, change),
            PendingPushResult::AbandonAndRetry => {}
        }

        Some(result)
    }

    /// The service a BuildKit step line belongs to, so a step from another
    /// service interrupts an open block like any other explicit source.
    fn pending_buildkit_source(&self, line: &str) -> Option<String> {
        let buildkit = parse_buildkit_step_line(line)?;
        buildkit
            .source
            .or_else(|| self.buildkit_steps.get(&buildkit.step_id).cloned())
    }

    /// Returns the sequence of the existing summary a property-block header
    /// belongs to.
    ///
    /// A source-less header only considers the newest event. A sourced header
    /// considers the newest event of the same source (ASCII-case-insensitive,
    /// compared with the promoted event source) within the last
    /// [`MAX_SOURCED_TARGET_LOOKBACK`] events, so interleaved output from other
    /// sources does not break folding. It never searches past a newer
    /// same-source event that does not match.
    ///
    /// Attachment is heuristic: the header and event are matched only on the
    /// time-of-day timestamp string and level (plus source). Two same-level
    /// events logged in the same millisecond can therefore mis-attach; this is
    /// an accepted limitation.
    fn property_target_sequence(&self, header: &PropertyFoldHeader) -> Option<u64> {
        let event = match header.source.as_deref() {
            Some(source) => self
                .events
                .iter()
                .rev()
                .take(MAX_SOURCED_TARGET_LOOKBACK)
                .find(|event| event.source.eq_ignore_ascii_case(source))?,
            None => self.events.back()?,
        };

        header.matches(event).then_some(event.sequence)
    }

    fn apply_pending_properties(
        &mut self,
        pending: PendingPropertyBlock,
        change: &mut BufferChange,
    ) {
        let Some(properties) = self.interpreter.property_object(&pending.lines.join("\n")) else {
            return;
        };

        // A sourced block's target may be followed by other sources' events,
        // so it is not necessarily at the back.
        if let Some(position) = self.index_of_sequence(pending.target_sequence)
            && let Some(event) = self.events.get_mut(position)
        {
            self.interpreter.apply_properties(event, properties.clone());
            change.updated.push(pending.target_sequence);
        }

        if pending.deferred_header {
            self.completed_property_blocks
                .push_back(CompletedPropertyBlock {
                    header_sequence: pending.target_sequence,
                    header: pending.header,
                    properties,
                });
            self.trim_completed_property_blocks();
        }
    }

    /// Moves deferred property blocks onto the newly appended back event and
    /// drops each block's standalone header event.
    ///
    /// A sourced block is decided by the first later event of its own source:
    /// it moves there when the event matches its header and is discarded
    /// otherwise, so other sources' events in between are ignored. A
    /// source-less block waits for the first later matching event, and at most
    /// one source-less block moves onto any event. Matching uses the same
    /// heuristic as [`Self::property_target_sequence`].
    fn apply_completed_property_block_to_back(&mut self, change: &mut BufferChange) {
        if self.completed_property_blocks.is_empty() {
            return;
        }
        let Some(event) = self.events.back() else {
            return;
        };

        let mut decided = Vec::new();
        let mut matched_source_less = false;
        for (position, block) in self.completed_property_blocks.iter().enumerate() {
            if block.header_sequence == event.sequence {
                continue;
            }

            match block.header.source.as_deref() {
                Some(source) if event.source.eq_ignore_ascii_case(source) => {
                    decided.push((position, block.header.matches(event)));
                }
                Some(_) => {}
                None if !matched_source_less && block.header.matches(event) => {
                    matched_source_less = true;
                    decided.push((position, true));
                }
                None => {}
            }
        }
        if decided.is_empty() {
            return;
        }

        let target_sequence = event.sequence;
        let mut attached = Vec::new();
        for (position, attach) in decided.into_iter().rev() {
            if let Some(block) = self.completed_property_blocks.remove(position)
                && attach
            {
                attached.push(block);
            }
        }
        // Attach oldest first so the newest block's properties win.
        for block in attached.into_iter().rev() {
            self.attach_completed_property_block(block, target_sequence, change);
        }
    }

    fn attach_completed_property_block(
        &mut self,
        block: CompletedPropertyBlock,
        target_sequence: u64,
        change: &mut BufferChange,
    ) {
        if let Some(event) = self
            .events
            .back_mut()
            .filter(|event| event.sequence == target_sequence)
        {
            self.interpreter.apply_properties(event, block.properties);
            change.updated.push(target_sequence);
        }

        if let Some(position) = self.index_of_sequence(block.header_sequence)
            && let Some(event) = self.events.remove(position)
        {
            change.removed.push(event.sequence);
        }
    }

    fn trim_completed_property_blocks(&mut self) {
        while self.completed_property_blocks.len() > self.capacity {
            self.completed_property_blocks.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn events(&self) -> &VecDeque<LogEvent> {
        &self.events
    }

    /// Looks up an event by sequence in O(log n).
    ///
    /// Events are always stored in ascending sequence order, but sequences may
    /// have gaps (eviction from the front, or a property-block header removed
    /// from the middle), so an offset from the front sequence is not reliable.
    pub(crate) fn event_by_sequence(&self, sequence: u64) -> Option<&LogEvent> {
        self.events.get(self.index_of_sequence(sequence)?)
    }

    fn index_of_sequence(&self, sequence: u64) -> Option<usize> {
        self.events
            .binary_search_by_key(&sequence, |event| event.sequence)
            .ok()
    }
}

fn is_continuation_line(message: &str) -> bool {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return true;
    }

    if message.chars().next().is_some_and(char::is_whitespace) {
        return true;
    }

    trimmed.starts_with("at ")
        || trimmed.starts_with("Caused by:")
        || trimmed.starts_with("Suppressed:")
        || trimmed.starts_with("...")
        || looks_like_error_continuation(trimmed)
        || looks_like_structured_continuation(trimmed)
}

fn looks_like_error_continuation(trimmed: &str) -> bool {
    let Some((head, _)) = trimmed.split_once(':') else {
        return false;
    };

    head == "Error" || head.ends_with("Error") || head.ends_with("Exception")
}

fn looks_like_structured_continuation(trimmed: &str) -> bool {
    matches!(
        trimmed.chars().next(),
        Some('{' | '}' | '[' | ']' | ',' | ')')
    ) || looks_like_property_entry(trimmed)
}

fn looks_like_property_entry(trimmed: &str) -> bool {
    let Some((key, value)) = trimmed.split_once(':') else {
        return false;
    };

    if !looks_like_property_key(key.trim()) {
        return false;
    }

    let value = value.trim().trim_end_matches(',').trim();
    value
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '"' | '\'' | '{' | '['))
        || matches!(value, "true" | "false" | "null")
        || value
            .chars()
            .next()
            .is_some_and(|ch| ch == '-' || ch.is_ascii_digit())
}

fn looks_like_property_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }

    let double_quoted = key.starts_with('"') && key.ends_with('"');
    let single_quoted = key.starts_with('\'') && key.ends_with('\'');
    if double_quoted || single_quoted {
        return key.len() > 2;
    }

    key.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

impl PendingPropertyBlock {
    fn new(target_sequence: u64, header: PropertyFoldHeader, deferred_header: bool) -> Self {
        Self {
            target_sequence,
            header,
            deferred_header,
            lines: Vec::new(),
            bytes: 0,
            brace_depth: 0,
            saw_open: false,
        }
    }

    fn push_line(
        &mut self,
        interpreter: &LogInterpreter,
        line: &str,
        buildkit_source: Option<String>,
    ) -> PendingPushResult {
        if interpreter.property_block_header(line).is_some() {
            return PendingPushResult::AbandonAndRetry;
        }

        let mut parsed = interpreter.pending_property_line(line);
        if parsed.source.is_none() {
            parsed.source = buildkit_source;
        }
        if let (Some(block_source), Some(line_source)) =
            (self.header.source.as_deref(), parsed.source.as_deref())
            && !line_source.eq_ignore_ascii_case(block_source)
        {
            return PendingPushResult::AbandonAndRetry;
        }

        let track_braces = if self.saw_open {
            if parsed.starts_record || (parsed.source.is_some() && !parsed.is_property_body) {
                return PendingPushResult::AbandonAndRetry;
            }
            // Unprefixed stray text is kept for the tolerant object parser, but
            // its braces cannot close the block.
            parsed.is_property_body
        } else {
            let trimmed = parsed.message.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('{') {
                return PendingPushResult::AbandonAndRetry;
            }
            true
        };

        let Some(bytes) = self
            .bytes
            .checked_add(parsed.message.len())
            .and_then(|bytes| bytes.checked_add(1))
        else {
            return PendingPushResult::AbandonAndRetry;
        };
        if self.lines.len() >= MAX_PENDING_PROPERTY_LINES || bytes > MAX_PENDING_PROPERTY_BYTES {
            return PendingPushResult::AbandonAndRetry;
        }

        if track_braces {
            self.update_brace_depth(&parsed.message);
        }
        self.lines.push(parsed.message);
        self.bytes = bytes;
        if self.is_complete() {
            PendingPushResult::Complete
        } else {
            PendingPushResult::Accepted
        }
    }

    fn is_complete(&self) -> bool {
        self.saw_open && self.brace_depth <= 0
    }

    fn update_brace_depth(&mut self, line: &str) {
        let mut in_string = None;
        let mut escaped = false;

        for ch in line.chars() {
            if let Some(quote) = in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    in_string = None;
                }
                continue;
            }

            match ch {
                '"' | '\'' => in_string = Some(ch),
                '{' => {
                    self.saw_open = true;
                    self.brace_depth += 1;
                }
                '}' => self.brace_depth -= 1,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(buffer: &LogBuffer) -> Vec<&str> {
        buffer
            .events()
            .iter()
            .map(|event| event.source.as_str())
            .collect()
    }

    fn source_config(fields: &[&str]) -> SourceConfig {
        SourceConfig::with_fields(fields)
    }

    #[test]
    fn mixed_service_fixture_preserves_sources_properties_and_raw_summaries() {
        use crate::filter::{LogFilter, PropertyPredicate};
        use crate::model::Level;

        let lines = include_str!("../fixtures/mixed-service-investigation.log")
            .lines()
            .collect::<Vec<_>>();
        let mut buffer = LogBuffer::new(100);
        for line in &lines {
            buffer.push_line((*line).to_string());
        }

        assert_eq!(
            sources(&buffer),
            vec![
                "minio-init",
                "api",
                "worker",
                "api",
                "database",
                "worker",
                "api",
                "api",
                "worker",
                "database",
                "worker",
                "api",
            ]
        );
        // Only the property block folds; every other raw line remains unchanged.
        assert_eq!(
            buffer
                .events()
                .iter()
                .map(|event| event.raw.as_str())
                .collect::<Vec<_>>(),
            [&lines[..7], &lines[13..]].concat()
        );
        for (request_id, expected_sequences) in [
            ("fixture-failed", vec![1, 2, 4, 5, 6]),
            ("fixture-success", vec![7, 8, 9, 10, 11]),
            ("fixture-failed-extra", vec![3]),
        ] {
            let filter = LogFilter {
                property_includes: vec![PropertyPredicate::exact("requestId", request_id)],
                ..LogFilter::default()
            };
            assert_eq!(
                buffer
                    .events()
                    .iter()
                    .filter(|event| filter.matches(event))
                    .map(|event| event.sequence)
                    .collect::<Vec<_>>(),
                expected_sequences
            );
        }
        let database_error = &buffer.events()[4];
        assert_eq!(database_error.message, "job insert rejected");
        assert_eq!(database_error.level, Level::Error);
        assert_eq!(
            database_error
                .property("errorCode")
                .unwrap()
                .value
                .to_string(),
            "23503"
        );
        let failure = &buffer.events()[6];
        assert_eq!(failure.message, "request failed");
        assert_eq!(failure.level, Level::Error);
        assert_eq!(
            failure.property("statusCode").unwrap().value.to_string(),
            "500"
        );
        assert_eq!(
            failure.property("cause").unwrap().value.to_string(),
            "job insert rejected: missing synthetic parent"
        );
        let success = buffer.events().back().unwrap();
        assert_eq!(success.level, Level::Info);
        assert_eq!(
            success.property("statusCode").unwrap().value.to_string(),
            "201"
        );
    }

    #[test]
    fn retains_only_the_configured_number_of_lines() {
        let mut buffer = LogBuffer::new(3);

        buffer.push_line("api | one".to_string());
        buffer.push_line("api | two".to_string());
        buffer.push_line("api | three".to_string());
        buffer.push_line("api | four".to_string());

        let raws = buffer
            .events()
            .iter()
            .map(|event| event.raw.as_str())
            .collect::<Vec<_>>();
        let sequences = buffer
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();

        assert_eq!(raws, vec!["api | two", "api | three", "api | four"]);
        assert_eq!(sequences, vec![1, 2, 3]);
    }

    #[test]
    fn finds_events_by_sequence_after_eviction() {
        let mut buffer = LogBuffer::new(3);

        buffer.push_line("api | one".to_string());
        buffer.push_line("api | two".to_string());
        buffer.push_line("api | three".to_string());
        buffer.push_line("api | four".to_string());

        assert!(buffer.event_by_sequence(0).is_none());
        assert_eq!(buffer.event_by_sequence(1).unwrap().message, "two");
        assert_eq!(buffer.event_by_sequence(3).unwrap().message, "four");
    }

    #[test]
    fn finds_events_by_sequence_after_sequence_gap() {
        let mut buffer = LogBuffer::new(5);

        buffer.push_line("api | one".to_string());
        buffer.push_line("api | two".to_string());
        buffer.push_line("api | three".to_string());
        buffer.events.remove(1);

        assert_eq!(buffer.event_by_sequence(2).unwrap().message, "three");
    }

    #[test]
    fn finds_every_event_by_sequence_after_mid_deque_property_header_removal() {
        let mut buffer = LogBuffer::new(1_000);

        buffer.push_line("[api] INFO first".to_string());
        // The header has no matching predecessor, so it is kept as event 1
        // until the following matching event claims its properties.
        buffer.push_line("[api] [21:05:37.312] INFO (#140):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] requestId: \"abc-123\",".to_string());
        buffer.push_line("[api] }".to_string());
        let change = buffer.push_line("[api] 21:05:37.312 INFO http.request ok".to_string());
        assert_eq!(change.appended, Some(2));
        assert_eq!(change.removed, vec![1]);

        for index in 0..2_000 {
            buffer.push_line(format!("[api] INFO line {index}"));
        }

        let sequences = buffer
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
        for sequence in &sequences {
            assert_eq!(
                buffer.event_by_sequence(*sequence).unwrap().sequence,
                *sequence
            );
        }
        assert!(buffer.event_by_sequence(1).is_none());
        assert!(buffer.event_by_sequence(sequences[0] - 1).is_none());
        assert!(buffer.event_by_sequence(sequences[999] + 1).is_none());

        // A gap that stays inside the retained window.
        let mut buffer = LogBuffer::new(100);
        for index in 0..100 {
            buffer.push_line(format!("[api] INFO line {index}"));
        }
        buffer.events.remove(50);
        for sequence in (0..100).filter(|sequence| *sequence != 50) {
            assert_eq!(
                buffer.event_by_sequence(sequence).unwrap().message,
                format!("line {sequence}")
            );
        }
        assert!(buffer.event_by_sequence(50).is_none());
    }

    #[test]
    fn unattributed_build_steps_are_grouped_without_guessing_a_service() {
        let mut buffer = LogBuffer::new(20);
        for line in [
            "[api] INFO ready",
            "#7 CACHED",
            "#8 [internal] load build definition",
            "#9 [builder 1/2] RUN compile",
            "#10 [worker internal] load metadata",
            "#7 DONE 0.1s",
            "#10 CACHED",
            "plain output",
        ] {
            buffer.push_line(line.to_string());
        }
        assert_eq!(
            sources(&buffer),
            vec![
                "api", "build", "build", "build", "worker", "build", "worker", "unknown"
            ]
        );
        assert_eq!(buffer.events()[1].raw, "#7 CACHED");
        assert_eq!(buffer.events()[3].message, "#9 [builder 1/2] RUN compile");
    }

    #[test]
    fn buildkit_shell_pipelines_do_not_override_step_source() {
        let mut buffer = LogBuffer::new(10);
        for line in [
            "#35 [api stage-0 1/2] RUN printf ready | cat",
            "#36 [worker internal] load metadata",
            "#35 CACHED",
            "#36 DONE 0.1s",
            "[web] INFO application | message",
        ] {
            buffer.push_line(line.into());
        }
        assert_eq!(
            sources(&buffer),
            vec!["api", "worker", "api", "worker", "web"]
        );
        assert_eq!(
            buffer.events()[0].message,
            "#35 [stage-0 1/2] RUN printf ready | cat"
        );
        assert_eq!(buffer.events()[2].raw, "#35 CACHED");
    }

    #[test]
    fn inherits_source_for_buildkit_step_continuations() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line(
            "#35 [vev-statistics base 5/7] RUN --mount=type=secret,id=NODE_AUTH_TOKEN sh -c 'npm ci'"
                .to_string(),
        );
        buffer.push_line("#35 0.531 npm ci".to_string());

        assert_eq!(sources(&buffer), vec!["vev-statistics", "vev-statistics"]);
        assert_eq!(
            buffer.events()[0].message,
            "#35 [base 5/7] RUN --mount=type=secret,id=NODE_AUTH_TOKEN sh -c 'npm ci'"
        );
        assert_eq!(buffer.events()[1].message, "#35 0.531 npm ci");
    }

    #[test]
    fn inherits_source_for_buildkit_internal_step_continuations() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("#11 [vev-statistics internal] load metadata for node".to_string());
        buffer.push_line("#11 DONE 1.4s".to_string());

        assert_eq!(sources(&buffer), vec!["vev-statistics", "vev-statistics"]);
        assert_eq!(
            buffer.events()[0].message,
            "#11 [internal] load metadata for node"
        );
        assert_eq!(buffer.events()[1].message, "#11 DONE 1.4s");
    }

    #[test]
    fn inherits_source_for_unprefixed_stack_continuations() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[backend] ERROR failed".to_string());
        buffer.push_line("    at handler (/app/src/main.ts:10:3)".to_string());
        buffer.push_line("Caused by: TypeError: missing user".to_string());

        assert_eq!(sources(&buffer), vec!["backend", "backend", "backend"]);
        assert_eq!(
            buffer.events()[1].message,
            "    at handler (/app/src/main.ts:10:3)"
        );
        assert_eq!(
            buffer.events()[2].message,
            "Caused by: TypeError: missing user"
        );
    }

    #[test]
    fn inherits_source_for_unprefixed_structured_continuations() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] INFO request completed".to_string());
        buffer.push_line("{".to_string());
        buffer.push_line("requestId: \"abc-123\",".to_string());
        buffer.push_line("}".to_string());

        assert_eq!(sources(&buffer), vec!["api", "api", "api", "api"]);
    }

    #[test]
    fn standalone_unprefixed_lines_reset_source_inheritance() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[backend] INFO ready".to_string());
        buffer.push_line("VITE ready in 200 ms".to_string());
        buffer.push_line("  plugin ready".to_string());

        assert_eq!(sources(&buffer), vec!["backend", "unknown", "unknown"]);
    }

    #[test]
    fn explicit_sources_update_inherited_source_context() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[backend] ERROR failed".to_string());
        buffer.push_line("    at backend_handler".to_string());
        buffer.push_line("[frontend] ERROR failed".to_string());
        buffer.push_line("    at frontend_handler".to_string());

        assert_eq!(
            sources(&buffer),
            vec!["backend", "backend", "frontend", "frontend"]
        );
    }

    #[test]
    fn promotes_default_source_fields_from_inline_properties() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("INFO ready service=api".to_string());
        buffer.push_line("INFO ready app=web".to_string());

        assert_eq!(sources(&buffer), vec!["api", "web"]);
    }

    #[test]
    fn promotes_source_field_from_json_properties() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line(
            r#"{"level":"info","message":"ready","service":"vev-mcp","module":"Function"}"#
                .to_string(),
        );

        assert_eq!(sources(&buffer), vec!["vev-mcp"]);
        assert_eq!(buffer.events()[0].message, "ready");
    }

    #[test]
    fn promotes_source_field_from_embedded_json_log_properties() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line(
            r#"{"log":"{\"level\":\"info\",\"message\":\"ready\",\"service\":\"vev-mcp\"}\n","stream":"stdout","time":"2026-06-04T13:00:20Z"}"#.to_string(),
        );

        assert_eq!(sources(&buffer), vec!["vev-mcp"]);
        assert_eq!(buffer.events()[0].message, "ready");
    }

    #[test]
    fn promotes_configured_source_fields_before_default_fields() {
        let mut buffer = LogBuffer::with_source_config(10, source_config(&["logger", "service"]));

        buffer.push_line("INFO ready service=backend logger=api".to_string());

        assert_eq!(sources(&buffer), vec!["api"]);
    }

    #[test]
    fn promotes_quoted_inline_source_field_values() {
        let mut buffer = LogBuffer::with_source_config(10, source_config(&["service"]));

        buffer.push_line("INFO ready service=\"api server\"".to_string());

        assert_eq!(sources(&buffer), vec!["api server"]);
    }

    #[test]
    fn explicit_source_prefix_wins_over_inline_source_fields() {
        let mut buffer = LogBuffer::with_source_config(10, source_config(&["service"]));

        buffer.push_line("[frontend] INFO ready service=backend".to_string());

        assert_eq!(sources(&buffer), vec!["frontend"]);
    }

    #[test]
    fn merges_property_block_into_previous_structured_event() {
        let mut buffer = LogBuffer::new(10);

        buffer
            .push_line("14:06:58.892 INFO http.request GET /api/v1/inventory 200 96ms".to_string());
        buffer.push_line("[14:06:58.892] INFO (#147):".to_string());
        buffer.push_line("  {".to_string());
        buffer.push_line("    messageKey: \"http.request\",".to_string());
        buffer.push_line("    statusCode: 200,".to_string());
        buffer.push_line("  }".to_string());

        assert_eq!(buffer.events().len(), 1);
        let event = buffer.events().back().unwrap();
        assert_eq!(event.message, "http.request GET /api/v1/inventory 200 96ms");
        assert_eq!(event.properties.len(), 2);
        assert_eq!(event.properties[0].key, "messageKey");
        assert_eq!(event.properties[1].key, "statusCode");
    }

    #[test]
    fn merges_prefixed_property_block_into_previous_structured_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[backend] 14:06:58.892 INFO http.request ok".to_string());
        buffer.push_line("[backend] [14:06:58.892] INFO (#147):".to_string());
        buffer.push_line("{".to_string());
        buffer.push_line("requestId: \"abc-123\",".to_string());
        buffer.push_line("}".to_string());

        assert_eq!(buffer.events().len(), 1);
        let event = buffer.events().back().unwrap();
        assert_eq!(event.source, "backend");
        assert_eq!(event.message, "http.request ok");
        assert_eq!(event.properties.len(), 1);
        assert_eq!(event.properties[0].key, "requestId");
    }

    #[test]
    fn merges_bracket_prefixed_property_block_body_into_previous_structured_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 21:05:37.312 INFO http.request ok".to_string());
        buffer.push_line("[api] [21:05:37.312] INFO (#140):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] messageKey: \"http.request\",".to_string());
        buffer.push_line("[api] statusCode: 200,".to_string());
        buffer.push_line("[api] }".to_string());

        assert_eq!(buffer.events().len(), 1);
        let event = buffer.events().back().unwrap();
        assert_eq!(event.source, "api");
        assert_eq!(event.message, "http.request ok");
        assert_eq!(event.properties.len(), 2);
        assert_eq!(event.properties[0].key, "messageKey");
        assert_eq!(event.properties[1].key, "statusCode");
    }

    #[test]
    fn merges_compose_prefixed_property_block_body_into_previous_structured_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("api | 21:05:37.312 INFO http.request ok".to_string());
        buffer.push_line("api | [21:05:37.312] INFO (#140):".to_string());
        buffer.push_line("api | {".to_string());
        buffer.push_line("api | requestId: \"abc-123\",".to_string());
        buffer.push_line("api | statusCode: 200,".to_string());
        buffer.push_line("api | }".to_string());

        assert_eq!(buffer.events().len(), 1);
        let event = buffer.events().back().unwrap();
        assert_eq!(event.source, "api");
        assert_eq!(event.message, "http.request ok");
        assert_eq!(event.properties.len(), 2);
        assert_eq!(event.properties[0].key, "requestId");
        assert_eq!(event.properties[1].key, "statusCode");
    }

    #[test]
    fn moves_property_block_onto_following_structured_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] [21:05:37.312] INFO (#140):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] requestId: \"89698d63\",".to_string());
        buffer.push_line("[api] statusCode: 200,".to_string());
        buffer.push_line("[api] }".to_string());
        buffer.push_line("[api] 21:05:37.312 INFO http.request ok".to_string());

        assert_eq!(buffer.events().len(), 1);
        let event = buffer.events().back().unwrap();
        assert_eq!(event.source, "api");
        assert_eq!(event.message, "http.request ok");
        assert_eq!(event.properties.len(), 2);
        assert_eq!(event.properties[0].key, "requestId");
        assert_eq!(event.properties[1].key, "statusCode");
    }

    #[test]
    fn promotes_source_fields_from_merged_property_blocks() {
        let mut buffer = LogBuffer::with_source_config(10, source_config(&["service"]));

        buffer.push_line("14:06:58.892 INFO http.request ok".to_string());
        buffer.push_line("[14:06:58.892] INFO (#147):".to_string());
        buffer.push_line("{".to_string());
        buffer.push_line("service: \"api\",".to_string());
        buffer.push_line("}".to_string());

        assert_eq!(buffer.events().len(), 1);
        assert_eq!(buffer.events().back().unwrap().source, "api");
    }

    #[test]
    fn keeps_unmatched_property_header_as_a_visible_line() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[14:06:58.892] INFO (#147):".to_string());

        assert_eq!(buffer.events().len(), 1);
        assert_eq!(
            buffer.events().back().unwrap().raw,
            "[14:06:58.892] INFO (#147):"
        );
    }

    #[test]
    fn keeps_following_non_object_line_visible_after_unmatched_property_header() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[14:06:58.892] INFO (#147):".to_string());
        buffer.push_line("VITE ready in 200 ms".to_string());

        let raws = buffer
            .events()
            .iter()
            .map(|event| event.raw.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            raws,
            vec!["[14:06:58.892] INFO (#147):", "VITE ready in 200 ms"]
        );
    }

    #[test]
    fn sourced_after_summary_block_ignores_interleaved_equal_timestamp_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO api summary".to_string());
        buffer.push_line("[worker] 10:00:00.000 INFO worker summary".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] owner: api,".to_string());
        buffer.push_line("[api] }".to_string());

        assert_eq!(buffer.events().len(), 2);
        assert_eq!(buffer.events()[0].source, "api");
        assert_eq!(
            buffer.events()[0]
                .property("owner")
                .unwrap()
                .value
                .to_string(),
            "api"
        );
        assert!(buffer.events()[1].property("owner").is_none());
    }

    #[test]
    fn sourced_before_summary_block_ignores_interleaved_equal_timestamp_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] owner: api,".to_string());
        buffer.push_line("[api] }".to_string());
        buffer.push_line("[worker] 10:00:00.000 INFO worker summary".to_string());
        buffer.push_line("[api] 10:00:00.000 INFO api summary".to_string());

        assert_eq!(buffer.events().len(), 2);
        assert_eq!(buffer.events()[0].source, "worker");
        assert!(buffer.events()[0].property("owner").is_none());
        assert_eq!(buffer.events()[1].source, "api");
        assert_eq!(
            buffer.events()[1]
                .property("owner")
                .unwrap()
                .value
                .to_string(),
            "api"
        );
    }

    #[test]
    fn sourced_after_summary_block_does_not_search_past_newer_same_source_mismatch() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO stale target".to_string());
        buffer.push_line("[api] 10:00:01.000 INFO newer event".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] { owner: \"api\" }".to_string());

        assert_eq!(buffer.events().len(), 3);
        assert!(buffer.events()[0].property("owner").is_none());
        assert!(buffer.events()[1].property("owner").is_none());
        assert_eq!(buffer.events()[2].raw, "[api] [10:00:00.000] INFO (#1):");
        assert_eq!(
            buffer.events()[2]
                .property("owner")
                .unwrap()
                .value
                .to_string(),
            "api"
        );
    }

    #[test]
    fn sourced_before_summary_block_expires_on_first_same_source_mismatch() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] { owner: api }".to_string());
        buffer.push_line("[worker] 10:00:00.000 INFO ignored other source".to_string());
        buffer.push_line("[api] 10:00:01.000 INFO deciding mismatch".to_string());
        buffer.push_line("[api] 10:00:00.000 INFO too late".to_string());

        assert_eq!(buffer.events().len(), 4);
        assert_eq!(buffer.events()[0].raw, "[api] [10:00:00.000] INFO (#1):");
        assert!(buffer.events()[0].property("owner").is_some());
        assert!(buffer.events()[3].property("owner").is_none());
    }

    #[test]
    fn sourced_property_matching_is_ascii_case_insensitive() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[API] 10:00:00.000 INFO summary".to_string());
        buffer.push_line("api | [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("Api | { requestId: \"one\" }".to_string());

        assert_eq!(buffer.events().len(), 1);
        assert_eq!(buffer.events()[0].source, "API");
        assert_eq!(
            buffer.events()[0]
                .property("requestId")
                .unwrap()
                .value
                .to_string(),
            "one"
        );
    }

    #[test]
    fn sourced_property_matching_uses_promoted_canonical_event_source() {
        let mut after = LogBuffer::new(10);
        after.push_line("10:00:00.000 INFO summary service=api".to_string());
        after.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        after.push_line("[api] { requestId: \"after\" }".to_string());

        assert_eq!(after.events().len(), 1);
        assert_eq!(after.events()[0].source, "api");
        assert_eq!(
            after.events()[0]
                .property("requestId")
                .unwrap()
                .value
                .to_string(),
            "after"
        );

        let mut before = LogBuffer::new(10);
        before.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        before.push_line("[api] { requestId: \"before\" }".to_string());
        before.push_line("10:00:00.000 INFO summary service=api".to_string());

        assert_eq!(before.events().len(), 1);
        assert_eq!(before.events()[0].source, "api");
        assert_eq!(
            before.events()[0]
                .property("requestId")
                .unwrap()
                .value
                .to_string(),
            "before"
        );
    }

    #[test]
    fn source_less_blocks_preserve_immediate_back_and_skip_mismatch_legacy_rules() {
        let mut after = LogBuffer::new(10);
        after.push_line("[api] 10:00:00.000 INFO api summary".to_string());
        after.push_line("[worker] 10:00:00.000 INFO worker summary".to_string());
        after.push_line("[10:00:00.000] INFO (#1):".to_string());
        after.push_line("{ legacy: \"after\" }".to_string());

        assert!(after.events()[0].property("legacy").is_none());
        assert_eq!(
            after.events()[1]
                .property("legacy")
                .unwrap()
                .value
                .to_string(),
            "after"
        );

        let mut before = LogBuffer::new(10);
        before.push_line("[10:00:00.000] INFO (#1):".to_string());
        before.push_line("{ legacy: \"before\" }".to_string());
        before.push_line("[api] 10:00:01.000 WARN mismatch".to_string());
        before.push_line("[worker] 10:00:00.000 INFO eventual target".to_string());

        assert_eq!(before.events().len(), 2);
        assert_eq!(before.events()[1].source, "worker");
        assert_eq!(
            before.events()[1]
                .property("legacy")
                .unwrap()
                .value
                .to_string(),
            "before"
        );
    }

    #[test]
    fn malformed_fold_retries_timestamped_summary_once_and_continues() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO original".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] partial: true,".to_string());
        let change = buffer.push_line("[api] 10:00:01.000 ERROR recovered".to_string());
        buffer.push_line("[api] INFO later".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(buffer.events().len(), 3);
        assert_eq!(buffer.events()[1].message, "recovered");
        assert_eq!(buffer.events()[2].message, "later");
        assert!(buffer.events()[0].property("partial").is_none());
        assert_eq!(
            buffer
                .events()
                .iter()
                .filter(|event| event.raw.contains("recovered"))
                .count(),
            1
        );
    }

    #[test]
    fn bracketed_timestamp_record_is_a_pending_boundary() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("10:00:00.000 INFO original".to_string());
        buffer.push_line("[10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("{".to_string());
        buffer.push_line("partial: true,".to_string());
        let change = buffer.push_line("[10:00:01.000] ERROR recovered".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(buffer.events().len(), 2);
        assert_eq!(buffer.events()[1].raw, "[10:00:01.000] ERROR recovered");
        assert_eq!(buffer.events()[1].level, crate::model::Level::Error);
        assert!(buffer.events()[0].property("partial").is_none());
    }

    #[test]
    fn pending_fold_retries_conflicting_source_and_another_header() {
        let mut conflict = LogBuffer::new(10);
        conflict.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        let change = conflict.push_line("[worker] {".to_string());
        conflict.push_line("[worker] INFO continued".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(conflict.events()[1].source, "worker");
        assert_eq!(conflict.events()[1].message, "{");
        assert_eq!(conflict.events()[2].message, "continued");

        let mut status = LogBuffer::new(10);
        status.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        status.push_line("[api] {".to_string());
        let change = status.push_line("worker Started container".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(status.events()[1].source, "worker");
        assert_eq!(status.events()[1].message, "Started container");

        let mut buildkit = LogBuffer::new(10);
        buildkit.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buildkit.push_line("[api] {".to_string());
        let change = buildkit.push_line("#35 [worker internal] load metadata for node".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(buildkit.events()[1].source, "worker");
        assert_eq!(
            buildkit.events()[1].message,
            "#35 [internal] load metadata for node"
        );

        let mut buildkit_continuation = LogBuffer::new(10);
        buildkit_continuation.push_line("#35 [worker internal] load metadata for node".to_string());
        buildkit_continuation.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buildkit_continuation.push_line("[api] {".to_string());
        let change = buildkit_continuation.push_line("#35 DONE 1.4s".to_string());

        assert_eq!(change.appended, Some(2));
        assert_eq!(buildkit_continuation.events()[2].source, "worker");

        let mut headers = LogBuffer::new(10);
        headers.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        let change = headers.push_line("[worker] [10:00:01.000] WARN (#2):".to_string());
        headers.push_line("[worker] { owner: \"worker\" }".to_string());
        headers.push_line("[worker] 10:00:01.000 WARN summary".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(headers.events().len(), 2);
        assert_eq!(headers.events()[0].source, "api");
        assert_eq!(headers.events()[1].source, "worker");
        assert_eq!(
            headers.events()[1]
                .property("owner")
                .unwrap()
                .value
                .to_string(),
            "worker"
        );
    }

    #[test]
    fn pending_body_accepts_tolerant_entries_and_scalar_array_elements() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO summary".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] values: [".to_string());
        buffer.push_line("[api] \"first\",".to_string());
        buffer.push_line("[api] 7,".to_string());
        buffer.push_line("[api] true,".to_string());
        buffer.push_line("[api] null,".to_string());
        buffer.push_line("[api] ],".to_string());
        buffer.push_line("[api] [ ],".to_string());
        buffer.push_line("[api] reason: failed hard,".to_string());
        buffer.push_line("[api] error: \"failed\",".to_string());
        buffer.push_line("[api] info: true,".to_string());
        buffer.push_line("[api] }".to_string());

        assert_eq!(buffer.events().len(), 1);
        let event = &buffer.events()[0];
        assert_eq!(
            event.property("reason").unwrap().value.to_string(),
            "failed hard"
        );
        assert_eq!(event.property("error").unwrap().value.to_string(), "failed");
        assert_eq!(event.property("info").unwrap().value.to_string(), "true");
    }

    #[test]
    fn prefixed_one_line_object_completes_but_prefixed_recovery_text_does_not_close() {
        let mut complete = LogBuffer::new(10);
        complete.push_line("[api] 10:00:00.000 INFO summary".to_string());
        complete.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        complete.push_line("[api] { requestId: \"one\" }".to_string());

        assert_eq!(complete.events().len(), 1);
        assert_eq!(
            complete.events()[0]
                .property("requestId")
                .unwrap()
                .value
                .to_string(),
            "one"
        );

        let mut recovered = LogBuffer::new(10);
        recovered.push_line("[api] 10:00:00.000 INFO summary".to_string());
        recovered.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        recovered.push_line("[api] {".to_string());
        recovered.push_line("[api] partial: true,".to_string());
        let change = recovered.push_line("[api] } recovered".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(recovered.events().len(), 2);
        assert_eq!(recovered.events()[1].message, "} recovered");
        assert!(recovered.events()[0].property("partial").is_none());
    }

    #[test]
    fn level_only_boundary_recovers_but_level_named_properties_remain_body_lines() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO summary".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] error: \"failed\",".to_string());
        buffer.push_line("[api] info: true,".to_string());
        let change = buffer.push_line("[api] ERROR recovered".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(buffer.events().len(), 2);
        assert!(buffer.events()[0].property("error").is_none());
        assert!(buffer.events()[0].property("info").is_none());
        assert_eq!(buffer.events()[1].message, "recovered");
    }

    #[test]
    fn pending_line_limit_accepts_exact_cap_and_retries_next_line() {
        let mut exact = LogBuffer::new(10);
        exact.push_line("10:00:00.000 INFO summary".to_string());
        exact.push_line("[10:00:00.000] INFO (#1):".to_string());
        exact.push_line("{".to_string());
        for _ in 0..(MAX_PENDING_PROPERTY_LINES - 2) {
            exact.push_line(String::new());
        }
        let change = exact.push_line("}".to_string());

        assert_eq!(change.updated, vec![0]);
        assert!(exact.pending_properties.is_none());
        assert_eq!(exact.events().len(), 1);

        let mut overflow = LogBuffer::new(10);
        overflow.push_line("10:00:00.000 INFO summary".to_string());
        overflow.push_line("[10:00:00.000] INFO (#1):".to_string());
        overflow.push_line("{".to_string());
        for _ in 0..(MAX_PENDING_PROPERTY_LINES - 1) {
            overflow.push_line(String::new());
        }
        let pending = overflow.pending_properties.as_ref().unwrap();
        assert_eq!(pending.lines.len(), MAX_PENDING_PROPERTY_LINES);
        assert!(pending.bytes <= MAX_PENDING_PROPERTY_BYTES);
        let change = overflow.push_line("}".to_string());

        assert_eq!(change.appended, Some(1));
        assert!(overflow.pending_properties.is_none());
        assert_eq!(overflow.events()[1].raw, "}");
    }

    #[test]
    fn pending_byte_limit_counts_newlines_blanks_and_utf8_exactly() {
        let mut measured = LogBuffer::new(10);
        measured.push_line("[10:00:00.000] INFO (#1):".to_string());
        measured.push_line("{".to_string());
        measured.push_line(String::new());
        measured.push_line("label: \"é\",".to_string());
        let pending = measured.pending_properties.as_ref().unwrap();
        assert_eq!(pending.lines, ["{", "", "label: \"é\","]);
        assert_eq!(
            pending.bytes,
            pending
                .lines
                .iter()
                .map(|line| line.len() + 1)
                .sum::<usize>()
        );
        assert_eq!(pending.bytes, 16);

        let mut exact = LogBuffer::new(10);
        exact.push_line("10:00:00.000 INFO summary".to_string());
        exact.push_line("[10:00:00.000] INFO (#1):".to_string());
        exact.push_line("{".to_string());
        let entry = format!("key: é{}", "x".repeat(MAX_PENDING_PROPERTY_BYTES - 12));
        assert_eq!(entry.len(), MAX_PENDING_PROPERTY_BYTES - 5);
        exact.push_line(entry);
        let change = exact.push_line("}".to_string());

        assert_eq!(change.updated, vec![0]);
        assert!(exact.pending_properties.is_none());
        assert_eq!(exact.events().len(), 1);

        let mut overflow = LogBuffer::new(10);
        overflow.push_line("10:00:00.000 INFO summary".to_string());
        overflow.push_line("[10:00:00.000] INFO (#1):".to_string());
        overflow.push_line("{".to_string());
        let oversized = format!("key: {}", "x".repeat(MAX_PENDING_PROPERTY_BYTES - 7));
        assert_eq!(oversized.len() + 3, MAX_PENDING_PROPERTY_BYTES + 1);
        let change = overflow.push_line(oversized.clone());

        assert_eq!(change.appended, Some(1));
        assert!(overflow.pending_properties.is_none());
        assert_eq!(overflow.events()[1].raw, oversized);
        assert!(overflow.events()[0].properties.is_empty());
    }

    #[test]
    fn eof_drops_incomplete_fold_without_applying_partial_properties() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO summary stable=true".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] partial: true,".to_string());
        assert!(buffer.pending_properties.is_some());

        buffer.finish_input();

        assert!(buffer.pending_properties.is_none());
        assert!(buffer.events()[0].property("stable").is_some());
        assert!(buffer.events()[0].property("partial").is_none());
    }

    #[test]
    fn closed_object_keeps_tolerant_property_parser_semantics() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[api] 10:00:00.000 INFO summary".to_string());
        buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buffer.push_line("[api] {".to_string());
        buffer.push_line("[api] valid: yes,".to_string());
        buffer.push_line("this line is ignored".to_string());
        buffer.push_line("[api] another: 2,".to_string());
        buffer.push_line("[api] }".to_string());

        assert_eq!(buffer.events().len(), 1);
        assert_eq!(
            buffer.events()[0]
                .property("valid")
                .unwrap()
                .value
                .to_string(),
            "yes"
        );
        assert_eq!(
            buffer.events()[0]
                .property("another")
                .unwrap()
                .value
                .to_string(),
            "2"
        );
    }

    #[test]
    fn sourced_header_target_search_is_bounded_to_recent_events() {
        let fold = |interleaved: usize| {
            let mut buffer = LogBuffer::new(1_000);
            buffer.push_line("[api] 10:00:00.000 INFO summary".to_string());
            for index in 0..interleaved {
                buffer.push_line(format!("[worker] INFO line {index}"));
            }
            buffer.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
            buffer.push_line("[api] { owner: \"api\" }".to_string());
            buffer
        };

        let within = fold(MAX_SOURCED_TARGET_LOOKBACK - 1);
        assert_eq!(within.len(), MAX_SOURCED_TARGET_LOOKBACK);
        assert!(within.events()[0].property("owner").is_some());

        // Past the bound the header is unmatched, so it stays visible and
        // keeps its own properties.
        let beyond = fold(MAX_SOURCED_TARGET_LOOKBACK);
        assert_eq!(beyond.len(), MAX_SOURCED_TARGET_LOOKBACK + 2);
        assert!(beyond.events()[0].property("owner").is_none());
        let header = beyond.events().back().unwrap();
        assert_eq!(header.raw, "[api] [10:00:00.000] INFO (#1):");
        assert!(header.property("owner").is_some());
    }
}
