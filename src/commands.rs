use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    Search,
    SourceFilter,
    LevelFilter,
    ToggleDetails,
    CopySelectedLine,
    StartVisualSelection,
    PreviousProperty,
    NextProperty,
    FollowProperty,
    AddMessageField,
    MessageFields,
    PropertyFilters,
    IncludeProperty,
    ExcludeProperty,
    ClearFilters,
    UndoFilterChange,
    SaveFilterPreset,
    FilterPresets,
    ExportVisibleLogs,
    ToggleMarker,
    Sources,
    NextMatch,
    PreviousMatch,
    ToggleFollow,
    JumpTop,
    JumpBottom,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub shortcut: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub action: CommandAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandHelpLevel {
    Full,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandHelpItem {
    pub shortcut: &'static str,
    pub label: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command {
        shortcut: "/",
        label: "Search",
        description: "Set text filter and search query",
        action: CommandAction::Search,
    },
    Command {
        shortcut: "s",
        label: "Source filter",
        description: "Filter rows by service or source",
        action: CommandAction::SourceFilter,
    },
    Command {
        shortcut: "l",
        label: "Level filter",
        description: "Filter rows by log level",
        action: CommandAction::LevelFilter,
    },
    Command {
        shortcut: "Enter",
        label: "Details",
        description: "Open or close details for the selected row",
        action: CommandAction::ToggleDetails,
    },
    Command {
        shortcut: "y",
        label: "Copy line",
        description: "Copy the selected raw log line to the clipboard",
        action: CommandAction::CopySelectedLine,
    },
    Command {
        shortcut: "v",
        label: "Visual copy",
        description: "Select visible log lines; press y to copy the range",
        action: CommandAction::StartVisualSelection,
    },
    Command {
        shortcut: "[",
        label: "Previous property",
        description: "Move to the previous property in details",
        action: CommandAction::PreviousProperty,
    },
    Command {
        shortcut: "]",
        label: "Next property",
        description: "Move to the next property in details",
        action: CommandAction::NextProperty,
    },
    Command {
        shortcut: "f",
        label: "Filter selected value",
        description: "Show rows with the selected property's exact value",
        action: CommandAction::FollowProperty,
    },
    Command {
        shortcut: "m",
        label: "Pin field column",
        description: "Pin the selected property as a stable log-row column",
        action: CommandAction::AddMessageField,
    },
    Command {
        shortcut: "M",
        label: "Pinned fields",
        description: "View, search, and remove pinned field columns",
        action: CommandAction::MessageFields,
    },
    Command {
        shortcut: "P",
        label: "Property filters",
        description: "View, search, edit, and delete property filters",
        action: CommandAction::PropertyFilters,
    },
    Command {
        shortcut: "+",
        label: "Show property filter",
        description: "Show rows matching a property filter",
        action: CommandAction::IncludeProperty,
    },
    Command {
        shortcut: "-",
        label: "Hide property filter",
        description: "Hide rows matching a property filter",
        action: CommandAction::ExcludeProperty,
    },
    Command {
        shortcut: "c",
        label: "Clear filters",
        description: "Remove search, source, level, and property filters",
        action: CommandAction::ClearFilters,
    },
    Command {
        shortcut: "u",
        label: "Undo filter",
        description: "Restore the previous filter state",
        action: CommandAction::UndoFilterChange,
    },
    Command {
        shortcut: "S",
        label: "Save filter preset",
        description: "Save the current filters as an in-session preset",
        action: CommandAction::SaveFilterPreset,
    },
    Command {
        shortcut: "V",
        label: "Filter presets",
        description: "Search and restore saved filter presets",
        action: CommandAction::FilterPresets,
    },
    Command {
        shortcut: "e",
        label: "Export visible logs",
        description: "Write the current visible log rows to loggle-export.log",
        action: CommandAction::ExportVisibleLogs,
    },
    Command {
        shortcut: "T",
        label: "Toggle marker",
        description: "Mark or unmark the selected log row",
        action: CommandAction::ToggleMarker,
    },
    Command {
        shortcut: "O",
        label: "Sources",
        description: "Show observed source counts and recent status",
        action: CommandAction::Sources,
    },
    Command {
        shortcut: "n",
        label: "Next match",
        description: "Jump to the next row matching the search query",
        action: CommandAction::NextMatch,
    },
    Command {
        shortcut: "N",
        label: "Previous match",
        description: "Jump to the previous row matching the search query",
        action: CommandAction::PreviousMatch,
    },
    Command {
        shortcut: "Space/p",
        label: "Pause or follow",
        description: "Toggle live following at the bottom",
        action: CommandAction::ToggleFollow,
    },
    Command {
        shortcut: "gg",
        label: "Top",
        description: "Jump to the first visible row",
        action: CommandAction::JumpTop,
    },
    Command {
        shortcut: "G",
        label: "Bottom",
        description: "Jump to the newest visible row and follow",
        action: CommandAction::JumpBottom,
    },
    Command {
        shortcut: "q",
        label: "Quit",
        description: "Exit loggle",
        action: CommandAction::Quit,
    },
];

const FULL_STATUS_HELP: &[CommandAction] = &[
    CommandAction::Quit,
    CommandAction::Search,
    CommandAction::SourceFilter,
    CommandAction::LevelFilter,
    CommandAction::ToggleDetails,
    CommandAction::PropertyFilters,
    CommandAction::ClearFilters,
];

const COMPACT_STATUS_HELP: &[CommandAction] = &[
    CommandAction::Quit,
    CommandAction::Search,
    CommandAction::ClearFilters,
];

const COMMANDS_HELP_ITEM: CommandHelpItem = CommandHelpItem {
    shortcut: "?",
    label: "commands",
};

pub fn command_for_action(action: CommandAction) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.action == action)
}

pub fn normal_action_for_key(key: KeyEvent) -> Option<CommandAction> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('d' | 'u'), KeyModifiers::CONTROL) => None,
        (KeyCode::Char('q'), _) => Some(CommandAction::Quit),
        (KeyCode::Char('G'), _) => Some(CommandAction::JumpBottom),
        (KeyCode::Enter, _) => Some(CommandAction::ToggleDetails),
        (KeyCode::Char('y'), _) => Some(CommandAction::CopySelectedLine),
        (KeyCode::Char('v'), _) => Some(CommandAction::StartVisualSelection),
        (KeyCode::Char('/'), _) => Some(CommandAction::Search),
        (KeyCode::Char('s'), _) => Some(CommandAction::SourceFilter),
        (KeyCode::Char('l'), _) => Some(CommandAction::LevelFilter),
        (KeyCode::Char('+'), _) => Some(CommandAction::IncludeProperty),
        (KeyCode::Char('-'), _) => Some(CommandAction::ExcludeProperty),
        (KeyCode::Char('f'), _) => Some(CommandAction::FollowProperty),
        (KeyCode::Char('m'), _) => Some(CommandAction::AddMessageField),
        (KeyCode::Char('M'), _) => Some(CommandAction::MessageFields),
        (KeyCode::Char('P'), _) => Some(CommandAction::PropertyFilters),
        (KeyCode::Char(']'), _) => Some(CommandAction::NextProperty),
        (KeyCode::Char('['), _) => Some(CommandAction::PreviousProperty),
        (KeyCode::Char('c'), _) => Some(CommandAction::ClearFilters),
        (KeyCode::Char('u'), _) => Some(CommandAction::UndoFilterChange),
        (KeyCode::Char('S'), _) => Some(CommandAction::SaveFilterPreset),
        (KeyCode::Char('V'), _) => Some(CommandAction::FilterPresets),
        (KeyCode::Char('e'), _) => Some(CommandAction::ExportVisibleLogs),
        (KeyCode::Char('T'), _) => Some(CommandAction::ToggleMarker),
        (KeyCode::Char('O'), _) => Some(CommandAction::Sources),
        (KeyCode::Char(' ' | 'p'), _) => Some(CommandAction::ToggleFollow),
        (KeyCode::Char('n'), _) => Some(CommandAction::NextMatch),
        (KeyCode::Char('N'), _) => Some(CommandAction::PreviousMatch),
        _ => None,
    }
}

pub fn status_help_items(level: CommandHelpLevel) -> impl Iterator<Item = CommandHelpItem> {
    let actions = match level {
        CommandHelpLevel::Full => FULL_STATUS_HELP,
        CommandHelpLevel::Compact => COMPACT_STATUS_HELP,
    };
    actions
        .iter()
        .filter_map(|action| help_item_for_action(*action))
        .chain(std::iter::once(COMMANDS_HELP_ITEM))
}

fn help_item_for_action(action: CommandAction) -> Option<CommandHelpItem> {
    let command = command_for_action(action)?;

    Some(CommandHelpItem {
        shortcut: command.shortcut,
        label: status_help_label(action),
    })
}

fn status_help_label(action: CommandAction) -> &'static str {
    match action {
        CommandAction::Search => "search",
        CommandAction::SourceFilter => "source",
        CommandAction::LevelFilter => "level",
        CommandAction::ToggleDetails => "details",
        CommandAction::PropertyFilters => "props",
        CommandAction::ClearFilters => "clear",
        CommandAction::Quit => "quit",
        _ => command_for_action(action)
            .map(|command| command.label)
            .unwrap_or("command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    #[test]
    fn normal_key_lookup_returns_catalog_actions() {
        assert_eq!(
            normal_action_for_key(key(KeyCode::Char('/'))),
            Some(CommandAction::Search)
        );
        assert_eq!(
            normal_action_for_key(key(KeyCode::Char('P'))),
            Some(CommandAction::PropertyFilters)
        );
        assert_eq!(
            normal_action_for_key(key(KeyCode::Char(' '))),
            Some(CommandAction::ToggleFollow)
        );
        assert_eq!(
            normal_action_for_key(key(KeyCode::Char('p'))),
            Some(CommandAction::ToggleFollow)
        );
        assert_eq!(
            normal_action_for_key(key(KeyCode::Enter)),
            Some(CommandAction::ToggleDetails)
        );
    }

    #[test]
    fn movement_control_keys_are_not_catalog_commands() {
        assert_eq!(normal_action_for_key(ctrl_key('d')), None);
        assert_eq!(normal_action_for_key(ctrl_key('u')), None);
    }

    #[test]
    fn status_help_uses_catalog_shortcuts() {
        let help = status_help_items(CommandHelpLevel::Full).collect::<Vec<_>>();

        assert_eq!(
            help,
            vec![
                CommandHelpItem {
                    shortcut: "q",
                    label: "quit"
                },
                CommandHelpItem {
                    shortcut: "/",
                    label: "search"
                },
                CommandHelpItem {
                    shortcut: "s",
                    label: "source"
                },
                CommandHelpItem {
                    shortcut: "l",
                    label: "level"
                },
                CommandHelpItem {
                    shortcut: "Enter",
                    label: "details"
                },
                CommandHelpItem {
                    shortcut: "P",
                    label: "props"
                },
                CommandHelpItem {
                    shortcut: "c",
                    label: "clear"
                },
                CommandHelpItem {
                    shortcut: "?",
                    label: "commands"
                },
            ]
        );
    }
}
