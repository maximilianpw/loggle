#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    Search,
    SourceFilter,
    LevelFilter,
    ToggleDetails,
    PreviousProperty,
    NextProperty,
    FollowProperty,
    AddMessageField,
    MessageFields,
    PropertyFilters,
    IncludeProperty,
    ExcludeProperty,
    ClearFilters,
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
        label: "Add message field",
        description: "Append selected property values to log-row messages",
        action: CommandAction::AddMessageField,
    },
    Command {
        shortcut: "M",
        label: "Message fields",
        description: "View, search, and remove displayed message fields",
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
