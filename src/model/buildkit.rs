use super::{BuildKitStepLine, clean_display_text, compose::looks_like_source_token};

pub(crate) fn parse_buildkit_step_line(line: &str) -> Option<BuildKitStepLine> {
    let line = clean_display_text(line);
    let trimmed = line.trim_start();
    let after_hash = trimmed.strip_prefix('#')?;
    let id_end = after_hash
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))?;
    let step_number = &after_hash[..id_end];
    if step_number.is_empty() || !step_number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let step_id = format!("#{step_number}");
    let after_step = after_hash[id_end..].trim_start();
    let Some(after_open) = after_step.strip_prefix('[') else {
        return Some(BuildKitStepLine {
            step_id,
            source: None,
            message: trimmed.to_string(),
        });
    };

    let (context, after_context) = after_open.split_once(']')?;
    let context = context.trim();
    let source = buildkit_context_source(context);
    let message = buildkit_message_without_service(&step_id, context, after_context);

    Some(BuildKitStepLine {
        step_id,
        source,
        message,
    })
}

fn buildkit_context_source(context: &str) -> Option<String> {
    let parts = context.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }

    let source = parts[0];
    if !looks_like_source_token(source) || is_reserved_buildkit_source(source) {
        return None;
    }

    let second = parts[1];
    if second.eq_ignore_ascii_case("internal") {
        return Some(source.to_string());
    }

    (parts.len() >= 3
        && parts
            .last()
            .is_some_and(|part| looks_like_buildkit_step_count(part)))
    .then(|| source.to_string())
}

fn buildkit_message_without_service(step_id: &str, context: &str, after_context: &str) -> String {
    let mut context_parts = context.split_whitespace();
    let context_without_source = if buildkit_context_source(context).is_some() {
        context_parts.next();
        context_parts.collect::<Vec<_>>().join(" ")
    } else {
        context.to_string()
    };
    let after_context = after_context.trim_start();

    if context_without_source.is_empty() {
        format!("{step_id} {after_context}").trim_end().to_string()
    } else if after_context.is_empty() {
        format!("{step_id} [{context_without_source}]")
    } else {
        format!("{step_id} [{context_without_source}] {after_context}")
    }
}

fn is_reserved_buildkit_source(source: &str) -> bool {
    source.eq_ignore_ascii_case("internal") || source.eq_ignore_ascii_case("auth")
}

fn looks_like_buildkit_step_count(value: &str) -> bool {
    let Some((current, total)) = value.split_once('/') else {
        return false;
    };

    !current.is_empty()
        && !total.is_empty()
        && current.chars().all(|ch| ch.is_ascii_digit())
        && total.chars().all(|ch| ch.is_ascii_digit())
}
