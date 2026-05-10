use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Unknown,
}

impl Level {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "fatal" => Some(Self::Fatal),
            "error" | "err" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" | "log" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" | "verbose" => Some(Self::Trace),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Fatal => "fatal",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::Unknown => "unknown",
        };
        f.write_str(label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    pub sequence: u64,
    pub source: String,
    pub level: Level,
    pub raw: String,
    pub message: String,
}

impl LogEvent {
    pub fn from_line(sequence: u64, raw: String) -> Self {
        let parsed = parse_compose_line(&raw);
        let level = infer_level(&parsed.message);

        Self {
            sequence,
            source: parsed.source,
            level,
            raw,
            message: parsed.message,
        }
    }
}

pub fn parse_compose_line(line: &str) -> ParsedLine {
    if let Some(parsed) = parse_bracket_prefixed_line(line) {
        return parsed;
    }

    if let Some((source, message)) = line.split_once('|') {
        let source = clean_display_text(source.trim());
        if !source.is_empty() {
            return ParsedLine {
                source,
                message: clean_display_text(message.trim_start()),
            };
        }
    }

    ParsedLine {
        source: "unknown".to_string(),
        message: clean_display_text(line),
    }
}

fn parse_bracket_prefixed_line(line: &str) -> Option<ParsedLine> {
    let rest = line.strip_prefix('[')?;
    let (source, message) = rest.split_once(']')?;
    let source = clean_display_text(source.trim());

    (!source.is_empty()).then(|| ParsedLine {
        source,
        message: clean_display_text(message.trim_start()),
    })
}

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

pub fn infer_level(message: &str) -> Level {
    let tokens = message
        .split(|value: char| !value.is_ascii_alphanumeric())
        .filter_map(Level::parse)
        .collect::<Vec<_>>();

    if tokens.contains(&Level::Fatal) {
        Level::Fatal
    } else if tokens.contains(&Level::Error) {
        Level::Error
    } else if tokens.contains(&Level::Warn) {
        Level::Warn
    } else if tokens.contains(&Level::Info) {
        Level::Info
    } else if tokens.contains(&Level::Debug) {
        Level::Debug
    } else if tokens.contains(&Level::Trace) {
        Level::Trace
    } else {
        Level::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compose_line_with_standard_spacing() {
        let parsed = parse_compose_line("api | ERROR failed");

        assert_eq!(parsed.source, "api");
        assert_eq!(parsed.message, "ERROR failed");
    }

    #[test]
    fn parses_compose_line_with_spacing_variants() {
        let parsed = parse_compose_line("  worker  |    started");

        assert_eq!(parsed.source, "worker");
        assert_eq!(parsed.message, "started");
    }

    #[test]
    fn parses_concurrently_named_prefix() {
        let parsed = parse_compose_line("[frontend] VITE ready");

        assert_eq!(parsed.source, "frontend");
        assert_eq!(parsed.message, "VITE ready");
    }

    #[test]
    fn parses_concurrently_backend_prefix_with_level() {
        let event = LogEvent::from_line(
            0,
            "[backend] INFO http.request GET /api/v1/auth/me 200".to_string(),
        );

        assert_eq!(event.source, "backend");
        assert_eq!(event.message, "INFO http.request GET /api/v1/auth/me 200");
        assert_eq!(event.level, Level::Info);
    }

    #[test]
    fn parses_concurrently_padded_prefix() {
        let parsed = parse_compose_line("[backend ] ERROR failed");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "ERROR failed");
    }

    #[test]
    fn parses_concurrently_numeric_prefix() {
        let parsed = parse_compose_line("[0] started");

        assert_eq!(parsed.source, "0");
        assert_eq!(parsed.message, "started");
    }

    #[test]
    fn falls_back_to_unknown_for_raw_lines() {
        let parsed = parse_compose_line("plain line with no prefix");

        assert_eq!(parsed.source, "unknown");
        assert_eq!(parsed.message, "plain line with no prefix");
    }

    #[test]
    fn keeps_unprefixed_vite_and_api_lines_unknown() {
        let vite = parse_compose_line("VITE v5.4.0  ready in 200 ms");
        let api = parse_compose_line("GET /api/v1/auth/me 200");

        assert_eq!(vite.source, "unknown");
        assert_eq!(vite.message, "VITE v5.4.0  ready in 200 ms");
        assert_eq!(api.source, "unknown");
        assert_eq!(api.message, "GET /api/v1/auth/me 200");
    }

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

    #[test]
    fn infers_common_levels_case_insensitively() {
        assert_eq!(infer_level("FATAL crash"), Level::Fatal);
        assert_eq!(infer_level("ERROR failed"), Level::Error);
        assert_eq!(infer_level("Warning: retrying"), Level::Warn);
        assert_eq!(infer_level("info: listening"), Level::Info);
        assert_eq!(
            infer_level("[Nest] 32 - 05/08/2026, 4:18:15 PM LOG [NestFactory] Starting Nest application..."),
            Level::Info
        );
        assert_eq!(infer_level("debug details"), Level::Debug);
        assert_eq!(infer_level("trace span"), Level::Trace);
        assert_eq!(infer_level("verbose route mapped"), Level::Trace);
    }

    #[test]
    fn infers_unknown_without_level_tokens() {
        assert_eq!(infer_level("request completed"), Level::Unknown);
    }
}
