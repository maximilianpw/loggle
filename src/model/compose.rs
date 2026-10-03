use super::{
    ParsedLine, clean_display_text,
    structured::{looks_like_timestamp, split_first_token},
};

pub fn parse_compose_line(line: &str) -> ParsedLine {
    let line = clean_display_text(line);

    if let Some(parsed) = parse_bracket_prefixed_line(&line) {
        return parsed;
    }

    if let Some((source, message)) = line.split_once('|') {
        let source = source.trim().to_string();
        if !source.is_empty() {
            return ParsedLine {
                source,
                message: message.trim_start().to_string(),
                source_explicit: true,
            };
        }
    }

    if let Some(parsed) = parse_compose_status_line(&line) {
        return parsed;
    }

    ParsedLine {
        source: "unknown".to_string(),
        message: line,
        source_explicit: false,
    }
}

fn parse_bracket_prefixed_line(line: &str) -> Option<ParsedLine> {
    let rest = line.strip_prefix('[')?;
    let (source, message) = rest.split_once(']')?;
    let source = source.trim().to_string();

    (!source.is_empty()).then(|| ParsedLine {
        source,
        message: message.trim_start().to_string(),
        source_explicit: true,
    })
}

fn parse_compose_status_line(line: &str) -> Option<ParsedLine> {
    let trimmed = line.trim_start();
    let (source, rest) = split_first_token(trimmed)?;
    if !looks_like_source_token(source) {
        return None;
    }

    let rest = rest.trim_start();
    let (status, _) = split_first_token(rest)?;
    let status = status.trim_end_matches(':');
    if !COMPOSE_STATUS_TOKENS
        .iter()
        .any(|candidate| status.eq_ignore_ascii_case(candidate))
    {
        return None;
    }

    Some(ParsedLine {
        source: source.to_string(),
        message: rest.to_string(),
        source_explicit: true,
    })
}

pub(super) fn looks_like_source_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

const COMPOSE_STATUS_TOKENS: &[&str] = &[
    "Pulling",
    "Pulled",
    "Building",
    "Built",
    "Error",
    "Started",
    "Starting",
    "Healthy",
    "Waiting",
    "Exited",
    "Recreated",
    "Recreating",
    "Running",
    "Created",
    "Creating",
];

pub(crate) fn message_without_source_prefix(line: &str) -> String {
    let line = clean_display_text(line);

    if let Some((source, message)) = line.split_once('|')
        && !source.trim().is_empty()
    {
        return message.trim_start().to_string();
    }

    if let Some(rest) = line.strip_prefix('[')
        && let Some((source, message)) = rest.split_once(']')
    {
        let source = source.trim();
        if !source.is_empty() && !looks_like_timestamp(source) {
            return message.trim_start().to_string();
        }
    }

    line
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
        assert!(parsed.source_explicit);
    }

    #[test]
    fn parses_colored_concurrently_named_prefix() {
        let parsed = parse_compose_line("\u{1b}[36m[backend]\u{1b}[0m INFO ready");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "INFO ready");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn parses_colored_concurrently_padded_prefix() {
        let parsed = parse_compose_line("\u{1b}[35m[backend ]\u{1b}[0m ERROR failed");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "ERROR failed");
        assert!(parsed.source_explicit);
    }
    #[test]
    fn parses_concurrently_padded_prefix() {
        let parsed = parse_compose_line("[backend ] ERROR failed");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "ERROR failed");
    }

    #[test]
    fn parses_compose_status_lines_as_sources() {
        let parsed = parse_compose_line("vev-server-rest Pulling");

        assert_eq!(parsed.source, "vev-server-rest");
        assert_eq!(parsed.message, "Pulling");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn parses_compose_error_status_lines_as_sources() {
        let parsed = parse_compose_line(
            "vev-server-rest Error response from daemon: pull access denied for image",
        );

        assert_eq!(parsed.source, "vev-server-rest");
        assert_eq!(
            parsed.message,
            "Error response from daemon: pull access denied for image"
        );
        assert!(parsed.source_explicit);
    }

    #[test]
    fn keeps_non_status_plain_lines_unknown() {
        let parsed = parse_compose_line("vev-server-rest connected to postgres");

        assert_eq!(parsed.source, "unknown");
        assert_eq!(parsed.message, "vev-server-rest connected to postgres");
        assert!(!parsed.source_explicit);
    }

    #[test]
    fn parses_concurrently_numeric_prefix() {
        let parsed = parse_compose_line("[0] started");

        assert_eq!(parsed.source, "0");
        assert_eq!(parsed.message, "started");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn falls_back_to_unknown_for_raw_lines() {
        let parsed = parse_compose_line("plain line with no prefix");

        assert_eq!(parsed.source, "unknown");
        assert_eq!(parsed.message, "plain line with no prefix");
        assert!(!parsed.source_explicit);
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
}
