use ratatui::style::Color;

#[derive(Debug, Clone, Copy)]
pub(super) struct GraphiteTheme {
    pub background: Color,
    pub panel_alt: Color,
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub removed: Color,
    pub warning: Color,
    pub info: Color,
    pub debug: Color,
    pub trace: Color,
    pub unknown: Color,
    pub highlight: Color,
    pub line_number_bg: Color,
    pub line_number_fg: Color,
}

pub(super) const THEME: GraphiteTheme = GraphiteTheme {
    background: Color::Rgb(17, 19, 21),
    panel_alt: Color::Rgb(29, 33, 38),
    accent: Color::Rgb(213, 224, 234),
    text: Color::Rgb(242, 244, 246),
    muted: Color::Rgb(154, 164, 175),
    removed: Color::Rgb(240, 160, 160),
    warning: Color::Rgb(230, 207, 152),
    info: Color::Rgb(136, 211, 155),
    debug: Color::Rgb(127, 209, 255),
    trace: Color::Rgb(196, 155, 255),
    unknown: Color::Rgb(121, 133, 146),
    highlight: Color::Rgb(255, 224, 102),
    line_number_bg: Color::Rgb(20, 24, 27),
    line_number_fg: Color::Rgb(121, 133, 146),
};
