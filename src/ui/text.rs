use std::borrow::Cow;

pub(super) fn compact_whitespace(value: &str) -> Cow<'_, str> {
    if is_compact_whitespace(value) {
        return Cow::Borrowed(value);
    }

    let mut output = String::with_capacity(value.len());
    for part in value.split_whitespace() {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(part);
    }

    Cow::Owned(output)
}

pub(super) fn truncate_tail(value: &str, max: usize) -> Cow<'_, str> {
    if max == 0 {
        return Cow::Borrowed("");
    }

    if value.chars().count() <= max {
        return Cow::Borrowed(value);
    }

    if max == 1 {
        return Cow::Borrowed("~");
    }

    let mut output = value.chars().take(max - 1).collect::<String>();
    output.push('~');
    Cow::Owned(output)
}

fn is_compact_whitespace(value: &str) -> bool {
    if value.chars().next().is_some_and(char::is_whitespace)
        || value.chars().next_back().is_some_and(char::is_whitespace)
    {
        return false;
    }

    let mut previous_whitespace = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            if previous_whitespace || ch != ' ' {
                return false;
            }
            previous_whitespace = true;
        } else {
            previous_whitespace = false;
        }
    }

    true
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
