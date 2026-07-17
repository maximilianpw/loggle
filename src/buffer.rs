use std::collections::{HashMap, VecDeque};

use crate::model::{
    LogEvent, LogInterpreter, LogProperty, ParsedLine, PropertyBlockHeader, SourceConfig,
    StructuredLineKind, parse_buildkit_step_line,
};

pub(crate) const MAX_PENDING_PROPERTY_LINES: usize = 256;
pub(crate) const MAX_PENDING_PROPERTY_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<LogEvent>,
    pending_properties: Option<PendingPropertyBlock>,
    completed_property_blocks: VecDeque<CompletedPropertyBlock>,
    active_source: Option<String>,
    buildkit_steps: HashMap<String, String>,
    source_config: SourceConfig,
    interpreter: LogInterpreter,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BufferChange {
    pub(crate) appended: Option<u64>,
    pub(crate) removed: Vec<u64>,
    pub(crate) updated: Vec<u64>,
    pub(crate) raw_target: Option<u64>,
}

#[derive(Debug)]
struct PendingPropertyBlock {
    target_sequence: u64,
    header: PropertyBlockHeader,
    deferred_header: bool,
    lines: Vec<String>,
    bytes: usize,
    brace_depth: i32,
    saw_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPushResult {
    Accepted,
    Complete,
    AbortAndRetry,
}

#[derive(Debug)]
struct CompletedPropertyBlock {
    header_sequence: u64,
    header: PropertyBlockHeader,
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
            source_config,
            interpreter: LogInterpreter,
        }
    }

    pub(crate) fn push_line(&mut self, line: String) -> BufferChange {
        let mut change = BufferChange::default();
        match self.push_pending_property_line(&line, &mut change) {
            Some(PendingPushResult::Accepted | PendingPushResult::Complete) => return change,
            Some(PendingPushResult::AbortAndRetry) | None => {}
        }

        self.push_ordinary_line(line, &mut change);
        change
    }

    fn push_ordinary_line(&mut self, line: String, change: &mut BufferChange) {
        if let Some(header) = self.interpreter.property_block_header(&line) {
            if let Some(target_sequence) = self.property_target_sequence(&header) {
                change.raw_target = Some(target_sequence);
                self.pending_properties =
                    Some(PendingPropertyBlock::new(target_sequence, header, false));
                return;
            }

            if let Some(target_sequence) = self.push_event(line, change) {
                self.pending_properties =
                    Some(PendingPropertyBlock::new(target_sequence, header, true));
                return;
            }

            return;
        }

        self.push_event(line, change);
    }

    fn push_event(&mut self, line: String, change: &mut BufferChange) -> Option<u64> {
        let mut parsed = self.interpreter.parse_source_line(&line);
        self.apply_buildkit_source_context(&line, &mut parsed);
        self.apply_source_context(&mut parsed);

        if self.capacity == 0 {
            self.next_sequence += 1;
            return None;
        }

        if self.events.len() == self.capacity {
            if let Some(event) = self.events.pop_front() {
                change.removed.push(event.sequence);
            }
        }

        let sequence = self.next_sequence;
        let event = self.interpreter.event_from_source_line(
            self.next_sequence,
            line,
            parsed,
            &self.source_config,
        );
        self.next_sequence += 1;
        self.events.push_back(event);
        change.appended = Some(sequence);
        self.apply_completed_property_block_to_back(change);
        Some(sequence)
    }

    fn apply_buildkit_source_context(&mut self, line: &str, parsed: &mut ParsedLine) {
        if parsed.source_explicit {
            return;
        }

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
            parsed.message = buildkit.message;
            parsed.source_explicit = true;
        }
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
        let result = pending.push_line(line, self.interpreter, buildkit_source);
        match result {
            PendingPushResult::Accepted => {
                change.raw_target = Some(pending.target_sequence);
                self.pending_properties = Some(pending);
            }
            PendingPushResult::Complete => {
                change.raw_target = Some(pending.target_sequence);
                self.apply_pending_properties(pending, change);
            }
            PendingPushResult::AbortAndRetry => {}
        }

        Some(result)
    }

    fn pending_buildkit_source(&self, line: &str) -> Option<String> {
        let buildkit = parse_buildkit_step_line(line)?;
        buildkit
            .source
            .or_else(|| self.buildkit_steps.get(&buildkit.step_id).cloned())
    }

    fn property_target_sequence(&self, header: &PropertyBlockHeader) -> Option<u64> {
        let event = if let Some(source) = header.source.as_deref() {
            self.events
                .iter()
                .rev()
                .find(|event| event.source.eq_ignore_ascii_case(source))?
        } else {
            self.events.back()?
        };

        header_matches_event(header, event).then_some(event.sequence)
    }

    fn apply_pending_properties(
        &mut self,
        pending: PendingPropertyBlock,
        change: &mut BufferChange,
    ) {
        let Some(properties) = self.interpreter.property_object(&pending.lines.join("\n")) else {
            return;
        };

        if let Some(event) = self
            .events
            .iter_mut()
            .find(|event| event.sequence == pending.target_sequence)
        {
            self.interpreter
                .apply_properties(event, properties.clone(), &self.source_config);
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

    fn apply_completed_property_block_to_back(&mut self, change: &mut BufferChange) {
        let Some(event) = self.events.back().cloned() else {
            return;
        };

        let mut retained = VecDeque::with_capacity(self.completed_property_blocks.len());
        let mut decided = Vec::new();
        let mut matched_source_less = false;
        while let Some(block) = self.completed_property_blocks.pop_front() {
            let is_distinct_event = event.sequence != block.header_sequence;
            let should_decide = match block.header.source.as_deref() {
                Some(source) => is_distinct_event && event.source.eq_ignore_ascii_case(source),
                None => {
                    is_distinct_event
                        && !matched_source_less
                        && header_matches_event(&block.header, &event)
                }
            };

            if should_decide {
                if block.header.source.is_none() {
                    matched_source_less = true;
                }
                decided.push(block);
            } else {
                retained.push_back(block);
            }
        }
        self.completed_property_blocks = retained;

        for block in decided {
            if header_matches_event(&block.header, &event) {
                self.attach_completed_property_block(block, event.sequence, change);
            }
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
            .iter_mut()
            .find(|event| event.sequence == target_sequence)
        {
            self.interpreter
                .apply_properties(event, block.properties, &self.source_config);
            change.updated.push(target_sequence);
        }

        if let Some(position) = self
            .events
            .iter()
            .position(|event| event.sequence == block.header_sequence)
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

    pub(crate) fn event_by_sequence(&self, sequence: u64) -> Option<&LogEvent> {
        let first_sequence = self.events.front()?.sequence;
        let offset = sequence.checked_sub(first_sequence)?;
        if let Ok(index) = usize::try_from(offset) {
            if let Some(event) = self.events.get(index) {
                if event.sequence == sequence {
                    return Some(event);
                }
            }
        }

        self.events.iter().find(|event| event.sequence == sequence)
    }

    pub(crate) fn finish_input(&mut self) {
        self.pending_properties = None;
    }
}

fn header_matches_event(header: &PropertyBlockHeader, event: &LogEvent) -> bool {
    event.timestamp.as_deref() == Some(header.timestamp.as_str())
        && event.level == header.level
        && header
            .source
            .as_deref()
            .is_none_or(|source| event.source.eq_ignore_ascii_case(source))
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
    fn new(target_sequence: u64, header: PropertyBlockHeader, deferred_header: bool) -> Self {
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
        line: &str,
        interpreter: LogInterpreter,
        buildkit_source: Option<String>,
    ) -> PendingPushResult {
        if interpreter.property_block_header(line).is_some() {
            return PendingPushResult::AbortAndRetry;
        }

        let mut parsed = interpreter.pending_property_line(line);
        if parsed.source.is_none() {
            parsed.source = buildkit_source;
        }
        if self.header.source.as_deref().is_some_and(|source| {
            parsed
                .source
                .as_deref()
                .is_some_and(|line_source| !line_source.eq_ignore_ascii_case(source))
        }) {
            return PendingPushResult::AbortAndRetry;
        }

        let trimmed = parsed.message.trim();
        let track_braces = if !self.saw_open {
            if !trimmed.is_empty() && !trimmed.starts_with('{') {
                return PendingPushResult::AbortAndRetry;
            }
            true
        } else {
            let structured_boundary = parsed.structured == StructuredLineKind::Timestamped
                || (parsed.structured == StructuredLineKind::LevelOnly && !parsed.is_property_body);
            if structured_boundary || (parsed.source.is_some() && !parsed.is_property_body) {
                return PendingPushResult::AbortAndRetry;
            }
            parsed.is_property_body
        };

        let Some(line_bytes) = parsed.message.len().checked_add(1) else {
            return PendingPushResult::AbortAndRetry;
        };
        let Some(bytes) = self.bytes.checked_add(line_bytes) else {
            return PendingPushResult::AbortAndRetry;
        };
        if self.lines.len() >= MAX_PENDING_PROPERTY_LINES || bytes > MAX_PENDING_PROPERTY_BYTES {
            return PendingPushResult::AbortAndRetry;
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
        assert_eq!(change.raw_target, None);
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
        assert_eq!(change.raw_target, None);
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
        assert_eq!(change.raw_target, None);
        assert_eq!(conflict.events()[1].source, "worker");
        assert_eq!(conflict.events()[1].message, "{");
        assert_eq!(conflict.events()[2].message, "continued");

        let mut status = LogBuffer::new(10);
        status.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        status.push_line("[api] {".to_string());
        let change = status.push_line("worker Started container".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(change.raw_target, None);
        assert_eq!(status.events()[1].source, "worker");
        assert_eq!(status.events()[1].message, "Started container");

        let mut buildkit = LogBuffer::new(10);
        buildkit.push_line("[api] [10:00:00.000] INFO (#1):".to_string());
        buildkit.push_line("[api] {".to_string());
        let change = buildkit.push_line("#35 [worker internal] load metadata for node".to_string());

        assert_eq!(change.appended, Some(1));
        assert_eq!(change.raw_target, None);
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
        assert_eq!(change.raw_target, None);
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

        assert_eq!(change.raw_target, Some(0));
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

        assert_eq!(change.raw_target, None);
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

        assert_eq!(change.raw_target, Some(0));
        assert!(exact.pending_properties.is_none());
        assert_eq!(exact.events().len(), 1);

        let mut overflow = LogBuffer::new(10);
        overflow.push_line("10:00:00.000 INFO summary".to_string());
        overflow.push_line("[10:00:00.000] INFO (#1):".to_string());
        overflow.push_line("{".to_string());
        let oversized = format!("key: {}", "x".repeat(MAX_PENDING_PROPERTY_BYTES - 7));
        assert_eq!(oversized.len() + 3, MAX_PENDING_PROPERTY_BYTES + 1);
        let change = overflow.push_line(oversized.clone());

        assert_eq!(change.raw_target, None);
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
}
