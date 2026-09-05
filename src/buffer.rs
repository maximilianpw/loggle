use std::collections::{HashMap, VecDeque};

use crate::model::{
    LogEvent, LogInterpreter, LogProperty, ParsedLine, PropertyBlockHeader, SourceConfig,
    parse_buildkit_step_line,
};

type Context = (u64, Option<String>, String);
const MAX_CONTEXTS: usize = 128;
const MAX_BLOCK_BYTES: usize = 256 * 1024;
const MAX_BLOCK_LINES: usize = 1024;
const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<LogEvent>,
    retained_bytes: usize,
    pending_properties: HashMap<Context, PendingPropertyBlock>,
    context: Context,
    last_sequences: HashMap<Context, u64>,
    completed_property_blocks: VecDeque<CompletedPropertyBlock>,
    active_source: HashMap<u64, String>,
    buildkit_steps: HashMap<(u64, String), String>,
    source_config: SourceConfig,
    interpreter: LogInterpreter,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BufferChange {
    pub(crate) appended: Option<u64>,
    pub(crate) consumed_by: Option<u64>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) removed: Vec<u64>,
    pub(crate) updated: Vec<u64>,
}

#[derive(Debug)]
struct PendingPropertyBlock {
    target_sequence: u64,
    deferred_header: Option<PropertyBlockHeader>,
    lines: Vec<String>,
    bytes: usize,
    brace_depth: i32,
    saw_open: bool,
}

#[derive(Debug)]
struct CompletedPropertyBlock {
    context: Context,
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
            retained_bytes: 0,
            pending_properties: HashMap::new(),
            context: (0, None, String::new()),
            last_sequences: HashMap::new(),
            completed_property_blocks: VecDeque::new(),
            active_source: HashMap::new(),
            buildkit_steps: HashMap::new(),
            source_config,
            interpreter: LogInterpreter,
        }
    }

    #[cfg(any(test, feature = "perf-harness"))]
    pub(crate) fn push_line(&mut self, line: String) -> BufferChange {
        self.push_source_line(0, None, line)
    }

    pub(crate) fn push_source_line(
        &mut self,
        stream: u64,
        source: Option<String>,
        mut line: String,
    ) -> BufferChange {
        if line.len() > MAX_RECORD_BYTES {
            let mut end = MAX_RECORD_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            line.push_str(" [loggle: record truncated]");
            line.shrink_to_fit();
        }
        let parsed = self.interpreter.parse_source_line(&line);
        self.context = (
            stream,
            source,
            if parsed.source_explicit {
                parsed.source
            } else {
                String::new()
            },
        );
        let mut change = BufferChange::default();
        if !self.last_sequences.contains_key(&self.context)
            && self.last_sequences.len() >= MAX_CONTEXTS
        {
            // Forget inference, never retained records, at the context budget.
            self.last_sequences.clear();
            self.pending_properties.clear();
            self.completed_property_blocks.clear();
            self.active_source.clear();
            change.notice = Some("source context limit reached; multiline inference reset");
        }
        if self.push_pending_property_line(&line, &mut change) {
            return change;
        }

        if let Some(header) = self.interpreter.property_block_header(&line) {
            if let Some(target_sequence) = self.property_target_sequence(&header) {
                change.consumed_by = Some(target_sequence);
                self.pending_properties.insert(
                    self.context.clone(),
                    PendingPropertyBlock::new(target_sequence, None),
                );
                return change;
            }

            if let Some(target_sequence) = self.push_event(line, &mut change) {
                self.pending_properties.insert(
                    self.context.clone(),
                    PendingPropertyBlock::new(target_sequence, Some(header)),
                );
                return change;
            }

            return change;
        }

        self.push_event(line, &mut change);
        change
    }

    fn push_event(&mut self, line: String, change: &mut BufferChange) -> Option<u64> {
        let mut parsed = self.interpreter.parse_source_line(&line);
        self.apply_buildkit_source_context(&line, &mut parsed);
        self.apply_source_context(&mut parsed);
        if !parsed.source_explicit {
            if let Some(source) = &self.context.1 {
                parsed.source = source.clone();
                parsed.source_explicit = true;
            }
        }

        if self.capacity == 0 {
            self.next_sequence += 1;
            return None;
        }

        if self.events.len() == self.capacity {
            self.evict_front(change);
        }

        let sequence = self.next_sequence;
        let raw = match &self.context.1 {
            Some(source) => format!("[{source}] {line}"),
            None => line,
        };
        let event = self.interpreter.event_from_source_line(
            self.next_sequence,
            raw,
            parsed,
            &self.source_config,
        );
        self.next_sequence += 1;
        self.retained_bytes += event_bytes(&event);
        self.events.push_back(event);
        self.last_sequences.insert(self.context.clone(), sequence);
        change.appended = Some(sequence);
        self.apply_completed_property_block_to_back(change);
        self.trim_bytes(change);
        Some(sequence)
    }

    fn evict_front(&mut self, change: &mut BufferChange) {
        if let Some(event) = self.events.pop_front() {
            self.retained_bytes -= event_bytes(&event);
            self.pending_properties
                .retain(|_, pending| pending.target_sequence != event.sequence);
            self.completed_property_blocks
                .retain(|block| block.header_sequence != event.sequence);
            change.removed.push(event.sequence);
        }
    }

    fn trim_bytes(&mut self, change: &mut BufferChange) {
        // Page replay already reads a bounded on-disk snapshot and needs all
        // its groups to apply filters; the live viewer has a payload budget.
        if self.capacity != usize::MAX {
            while self.retained_bytes > MAX_RETAINED_BYTES {
                self.evict_front(change);
            }
        }
    }

    fn apply_buildkit_source_context(&mut self, line: &str, parsed: &mut ParsedLine) {
        if parsed.source_explicit {
            return;
        }

        let Some(buildkit) = parse_buildkit_step_line(line) else {
            return;
        };

        if let Some(source) = buildkit.source {
            if self.buildkit_steps.len() >= 1024 {
                self.buildkit_steps.clear();
            }
            self.buildkit_steps
                .insert((self.context.0, buildkit.step_id.clone()), source.clone());
            parsed.source = source;
            parsed.message = buildkit.message;
            parsed.source_explicit = true;
            return;
        }

        if let Some(source) = self.buildkit_steps.get(&(self.context.0, buildkit.step_id)) {
            parsed.source = source.clone();
            parsed.message = buildkit.message;
            parsed.source_explicit = true;
        }
    }

    fn apply_source_context(&mut self, parsed: &mut ParsedLine) {
        if parsed.source_explicit {
            if self.active_source.len() >= MAX_CONTEXTS
                && !self.active_source.contains_key(&self.context.0)
            {
                self.active_source.clear();
            }
            self.active_source
                .insert(self.context.0, parsed.source.clone());
            return;
        }

        if is_continuation_line(&parsed.message) {
            if let Some(source) = self.active_source.get(&self.context.0) {
                parsed.source = source.clone();
            }
        } else {
            self.active_source.remove(&self.context.0);
        }
    }

    fn push_pending_property_line(&mut self, line: &str, change: &mut BufferChange) -> bool {
        let Some(mut pending) = self.pending_properties.remove(&self.context) else {
            return false;
        };

        if self.event_by_sequence(pending.target_sequence).is_none() {
            return false;
        }
        if !pending.push_line(line) {
            if pending.saw_open {
                change.notice = Some("incomplete or oversized property block abandoned");
            }
            return false;
        }

        change.consumed_by = Some(pending.target_sequence);
        if pending.is_complete() {
            self.apply_pending_properties(pending, change);
        } else {
            self.pending_properties
                .insert(self.context.clone(), pending);
        }

        true
    }

    fn property_target_sequence(&self, header: &PropertyBlockHeader) -> Option<u64> {
        let event = self.event_by_sequence(*self.last_sequences.get(&self.context)?)?;
        (event.timestamp.as_deref() == Some(header.timestamp.as_str())
            && event.level == header.level)
            .then_some(event.sequence)
    }

    fn apply_pending_properties(
        &mut self,
        pending: PendingPropertyBlock,
        change: &mut BufferChange,
    ) {
        let Some(properties) = self.interpreter.property_object(&pending.lines.join("\n")) else {
            change.notice = Some("malformed property block ignored");
            return;
        };

        if let Ok(index) = self
            .events
            .binary_search_by_key(&pending.target_sequence, |event| event.sequence)
        {
            let event = &mut self.events[index];
            self.retained_bytes -= event_bytes(event);
            self.interpreter
                .apply_properties(event, properties.clone(), &self.source_config);
            self.retained_bytes += event_bytes(event);
            change.updated.push(pending.target_sequence);
        }

        if let Some(header) = pending.deferred_header {
            self.completed_property_blocks
                .push_back(CompletedPropertyBlock {
                    context: self.context.clone(),
                    header_sequence: pending.target_sequence,
                    header,
                    properties,
                });
            self.trim_completed_property_blocks();
        }
        self.trim_bytes(change);
    }

    fn apply_completed_property_block_to_back(&mut self, change: &mut BufferChange) {
        let Some(event) = self.events.back() else {
            return;
        };

        let Some(position) = self.completed_property_blocks.iter().position(|block| {
            block.context == self.context
                && event.sequence != block.header_sequence
                && event.timestamp.as_deref() == Some(block.header.timestamp.as_str())
                && event.level == block.header.level
        }) else {
            return;
        };

        let Some(block) = self.completed_property_blocks.remove(position) else {
            return;
        };
        let target_sequence = event.sequence;

        if let Some(event) = self.events.back_mut() {
            self.retained_bytes -= event_bytes(event);
            self.interpreter
                .apply_properties(event, block.properties, &self.source_config);
            self.retained_bytes += event_bytes(event);
            change.updated.push(target_sequence);
        }

        if let Ok(position) = self
            .events
            .binary_search_by_key(&block.header_sequence, |event| event.sequence)
        {
            if let Some(event) = self.events.remove(position) {
                self.retained_bytes -= event_bytes(&event);
                change.removed.push(event.sequence);
            }
        }
    }

    fn trim_completed_property_blocks(&mut self) {
        while self.completed_property_blocks.len() > self.capacity.min(MAX_CONTEXTS) {
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

        self.events
            .binary_search_by_key(&sequence, |event| event.sequence)
            .ok()
            .and_then(|index| self.events.get(index))
    }
}

fn event_bytes(event: &LogEvent) -> usize {
    std::mem::size_of::<LogEvent>()
        + event.raw.len()
        + event.message.len()
        + event.source.len()
        + event.timestamp.as_ref().map_or(0, String::len)
        + event
            .properties
            .iter()
            .map(|property| {
                std::mem::size_of::<LogProperty>()
                    + property.key.len()
                    + property.value.as_display_str().len()
            })
            .sum::<usize>()
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
    fn new(target_sequence: u64, deferred_header: Option<PropertyBlockHeader>) -> Self {
        Self {
            target_sequence,
            deferred_header,
            lines: Vec::new(),
            bytes: 0,
            brace_depth: 0,
            saw_open: false,
        }
    }

    fn push_line(&mut self, line: &str) -> bool {
        if self.lines.len() >= MAX_BLOCK_LINES
            || self.bytes.saturating_add(line.len()) > MAX_BLOCK_BYTES
        {
            return false;
        }
        // A new log header terminates a malformed, unclosed object.
        if LogInterpreter.property_block_header(line).is_some() {
            return false;
        }
        let line = LogInterpreter.message_without_source_prefix(line);
        let trimmed = line.trim();
        if self.saw_open && !is_continuation_line(&line) {
            return false;
        }
        if !self.saw_open && !trimmed.is_empty() && !trimmed.starts_with('{') {
            return false;
        }

        self.update_brace_depth(&line);
        self.bytes += line.len();
        self.lines.push(line);
        true
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
    fn does_not_guess_owner_of_unprefixed_multiplexed_property_body() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[backend] 14:06:58.892 INFO http.request ok".to_string());
        buffer.push_line("[backend] [14:06:58.892] INFO (#147):".to_string());
        buffer.push_line("{".to_string());
        buffer.push_line("requestId: \"abc-123\",".to_string());
        buffer.push_line("}".to_string());

        assert_eq!(buffer.events().len(), 4);
        let event = buffer.events().front().unwrap();
        assert_eq!(event.source, "backend");
        assert_eq!(event.message, "http.request ok");
        assert!(event.properties.is_empty());
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
    fn interleaved_compose_blocks_and_identical_headers_stay_isolated() {
        let mut buffer = LogBuffer::new(10);
        for line in [
            "api | 14:06:58.892 INFO api request",
            "web | 14:06:58.892 INFO web request",
            "api | [14:06:58.892] INFO (#1):",
            "web | [14:06:58.892] INFO (#2):",
            "api | {",
            "web | {",
            "api | tenant: \"a\"",
            "web | tenant: \"b\"",
            "api | }",
            "web | }",
        ] {
            buffer.push_line(line.into());
        }
        assert_eq!(buffer.len(), 2);
        assert_eq!(
            buffer.events()[0]
                .property("tenant")
                .unwrap()
                .value
                .as_display_str(),
            "a"
        );
        assert_eq!(
            buffer.events()[1]
                .property("tenant")
                .unwrap()
                .value
                .as_display_str(),
            "b"
        );
    }

    #[test]
    fn completed_blocks_and_partial_objects_do_not_cross_physical_streams() {
        let mut buffer = LogBuffer::new(10);
        for line in ["[14:06:58.892] INFO (#1):", "{", "tenant: \"a\"", "}"] {
            buffer.push_source_line(1, Some("api".into()), line.into());
        }
        buffer.push_source_line(
            2,
            Some("api".into()),
            "14:06:58.892 INFO other stream".into(),
        );
        assert!(buffer.events().back().unwrap().property("tenant").is_none());
        buffer.push_source_line(
            1,
            Some("api".into()),
            "14:06:58.892 INFO same stream".into(),
        );
        assert_eq!(buffer.len(), 2);
        assert!(buffer.events().back().unwrap().property("tenant").is_some());
        buffer.push_source_line(1, None, "[14:06:59.892] INFO (#1):".into());
        buffer.push_source_line(1, None, "{".into());
        let change = buffer.push_source_line(2, None, "INFO must remain visible".into());
        assert!(change.appended.is_some());
    }

    #[test]
    fn malformed_assembly_and_context_tables_are_bounded() {
        let mut buffer = LogBuffer::new(16);
        buffer.push_line("[14:06:58.892] INFO (#1):".into());
        buffer.push_line("{".into());
        for _ in 0..MAX_BLOCK_LINES * 2 {
            buffer.push_line("  x: 1,".into());
        }
        assert!(buffer.pending_properties.is_empty());
        assert!(buffer.len() <= 16);
        buffer.push_line("[14:06:58.893] INFO (#1):".into());
        buffer.push_line("{".into());
        buffer.push_line(format!("  x: \"{}\"", "x".repeat(MAX_BLOCK_BYTES)));
        assert!(buffer.pending_properties.is_empty());
        for index in 0..2048 {
            buffer.push_source_line(index, None, format!("#{} [api base 1/2] RUN task", index));
            assert!(buffer.last_sequences.len() <= MAX_CONTEXTS);
            assert!(buffer.active_source.len() <= MAX_CONTEXTS);
            assert!(buffer.buildkit_steps.len() <= 1024);
        }
    }

    #[test]
    fn byte_retention_evicts_large_records_before_the_line_limit() {
        let mut buffer = LogBuffer::new(1000);
        for _ in 0..150 {
            buffer.push_line("x".repeat(MAX_RECORD_BYTES));
            assert!(buffer.retained_bytes <= MAX_RETAINED_BYTES);
            assert_eq!(
                buffer.retained_bytes,
                buffer.events.iter().map(event_bytes).sum::<usize>()
            );
        }
        assert!(buffer.len() < 150);
        assert_eq!(buffer.events.back().unwrap().sequence, 149);
    }
}
