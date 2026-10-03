pub fn clean_display_text(input: &str) -> String {
    strip_control_chars(&strip_ansi_escapes(input))
}

fn strip_ansi_escapes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            output.push(ch);
            continue;
        }

        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for value in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&value) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(value) = chars.next() {
                    if value == '\u{7}' {
                        break;
                    }

                    if value == '\x1b' && chars.peek().copied() == Some('\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }

    output
}

fn strip_control_chars(input: &str) -> String {
    input
        .chars()
        .filter_map(|ch| match ch {
            '\t' => Some(' '),
            ch if ch.is_control() => None,
            ch => Some(ch),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::model::parse_compose_line;

    #[test]
    fn strips_ansi_sequences_from_parsed_messages() {
        let parsed = parse_compose_line(
            "nestjs-backend | \u{1b}[32m[Nest] 32 - \u{1b}[39m05/08/2026 LOG ready",
        );

        assert_eq!(parsed.source, "nestjs-backend");
        assert_eq!(parsed.message, "[Nest] 32 - 05/08/2026 LOG ready");
    }

    #[test]
    fn strips_cursor_control_sequences_from_parsed_messages() {
        let parsed = parse_compose_line(
            "nestjs-backend | \u{1b}[J\u{1b}[3J\u{1b}[H[\u{1b}[90m4:25:35 PM\u{1b}[0m] Starting compilation",
        );

        assert_eq!(parsed.message, "[4:25:35 PM] Starting compilation");
    }

    #[test]
    fn strips_carriage_returns_and_other_control_chars_from_parsed_messages() {
        let parsed = parse_compose_line("api | progress 10%\rprogress 20%\u{8}\tready");

        assert_eq!(parsed.message, "progress 10%progress 20% ready");
    }
}
