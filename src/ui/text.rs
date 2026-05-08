pub(super) fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn truncate_tail(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }

    if value.chars().count() <= max {
        return value.to_string();
    }

    if max == 1 {
        return "~".to_string();
    }

    value.chars().take(max - 1).collect::<String>() + "~"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_tail_keeps_short_values_unchanged() {
        assert_eq!(truncate_tail("api", 8), "api");
    }

    #[test]
    fn truncate_tail_marks_truncated_values() {
        assert_eq!(truncate_tail("very-long-service", 8), "very-lo~");
        assert_eq!(truncate_tail("api", 1), "~");
        assert_eq!(truncate_tail("api", 0), "");
    }
}
