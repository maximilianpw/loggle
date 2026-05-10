use std::collections::VecDeque;

use crate::model::{
    LogEvent, PropertyBlockHeader, parse_property_block_header, parse_property_object,
};

#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<LogEvent>,
    pending_properties: Option<PendingPropertyBlock>,
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
        if self.capacity == 0 {
            self.next_sequence += 1;
            return;
        }

        if self.events.len() == self.capacity {
            self.events.pop_front();
        }

        let event = LogEvent::from_line(self.next_sequence, line);
        self.next_sequence += 1;
        self.events.push_back(event);
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
