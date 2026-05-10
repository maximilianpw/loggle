use std::collections::VecDeque;

use crate::model::{
    LogEvent, ParsedLine, PropertyBlockHeader, parse_compose_line, parse_property_block_header,
    parse_property_object,
};

#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<LogEvent>,
    pending_properties: Option<PendingPropertyBlock>,
    active_source: Option<String>,
}

#[derive(Debug)]
struct PendingPropertyBlock {
    target_sequence: u64,
    lines: Vec<String>,
    brace_depth: i32,
    saw_open: bool,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            next_sequence: 0,
            events: VecDeque::with_capacity(capacity),
            pending_properties: None,
            active_source: None,
        }
    }

    pub fn push_line(&mut self, line: String) {
        if self.push_pending_property_line(&line) {
            return;
        }

        if let Some(header) = parse_property_block_header(&line) {
            if let Some(target_sequence) = self.property_target_sequence(&header) {
                self.pending_properties = Some(PendingPropertyBlock::new(target_sequence));
                return;
            }
        }

        self.push_event(line);
    }

    fn push_event(&mut self, line: String) {
        let mut parsed = parse_compose_line(&line);
        self.apply_source_context(&mut parsed);

        if self.capacity == 0 {
            self.next_sequence += 1;
            return;
        }

        if self.events.len() == self.capacity {
            self.events.pop_front();
        }

        let event = LogEvent::from_parsed_line(self.next_sequence, line, parsed);
        self.next_sequence += 1;
        self.events.push_back(event);
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

    fn push_pending_property_line(&mut self, line: &str) -> bool {
        let Some(mut pending) = self.pending_properties.take() else {
            return false;
        };

        pending.push_line(line);
        if pending.is_complete() {
            self.apply_pending_properties(pending);
        } else {
            self.pending_properties = Some(pending);
        }

        true
    }

    fn property_target_sequence(&self, header: &PropertyBlockHeader) -> Option<u64> {
        let event = self.events.back()?;
        (event.timestamp.as_deref() == Some(header.timestamp.as_str())
            && event.level == header.level)
        .then_some(event.sequence)
    }

    fn apply_pending_properties(&mut self, pending: PendingPropertyBlock) {
        let Some(properties) = parse_property_object(&pending.lines.join("\n")) else {
            return;
        };

        if let Some(event) = self
            .events
            .iter_mut()
            .find(|event| event.sequence == pending.target_sequence)
        {
            event.set_properties(properties);
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn events(&self) -> &VecDeque<LogEvent> {
        &self.events
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
    fn new(target_sequence: u64) -> Self {
        Self {
            target_sequence,
            lines: Vec::new(),
            brace_depth: 0,
            saw_open: false,
        }
    }

    fn push_line(&mut self, line: &str) {
        self.update_brace_depth(line);
        self.lines.push(line.to_string());
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
        assert_eq!(buffer.events()[2].message, "Caused by: TypeError: missing user");
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
    fn merges_property_block_into_previous_structured_event() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line(
            "14:06:58.892 INFO http.request GET /api/v1/inventory 200 96ms".to_string(),
        );
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
    fn keeps_unmatched_property_header_as_a_visible_line() {
        let mut buffer = LogBuffer::new(10);

        buffer.push_line("[14:06:58.892] INFO (#147):".to_string());

        assert_eq!(buffer.events().len(), 1);
        assert_eq!(
            buffer.events().back().unwrap().raw,
            "[14:06:58.892] INFO (#147):"
        );
    }
}
