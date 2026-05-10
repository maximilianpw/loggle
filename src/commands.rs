#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    Search,
    SourceFilter,
    LevelFilter,
    ToggleDetails,
    PreviousProperty,
    NextProperty,
    FollowProperty,
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
        label: "Follow property",
        description: "Include the selected property's exact value",
        action: CommandAction::FollowProperty,
    },
    Command {
        shortcut: "+",
        label: "Include property filter",
        description: "Add an include filter for a property",
        action: CommandAction::IncludeProperty,
    },
    Command {
        shortcut: "-",
        label: "Exclude property filter",
        description: "Add an exclude filter for a property",
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
