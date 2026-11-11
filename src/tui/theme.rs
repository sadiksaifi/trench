//! Semantic styles for the TUI.
//!
//! Views ask for a role (`muted`, `selection`, `danger`) and never for a
//! color. The default theme uses only the terminal's own colors and
//! attributes, so it stays readable on light, dark and monochrome terminals.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Ordinary text.
    pub text: Style,
    /// Secondary text: labels, placeholders, tags, hint descriptions.
    pub muted: Style,
    /// Titles, the cursor, and the focused field.
    pub accent: Style,
    /// The selected row.
    pub selection: Style,
    pub success: Style,
    pub error: Style,
    pub warning: Style,
    /// The key that confirms a destructive action.
    pub danger: Style,
    pub border: Style,
    /// A key that does nothing right now.
    pub disabled: Style,
    /// Screen background. `Color::Reset` keeps the terminal's own.
    pub surface: Color,
    /// Dialog background. `Color::Reset` keeps the terminal's own.
    pub dialog_surface: Color,
}

impl Theme {
    /// `style` drawn on the screen background.
    pub fn on_surface(&self, style: Style) -> Style {
        on(style, self.surface)
    }

    /// `style` drawn on the dialog background.
    pub fn on_dialog(&self, style: Style) -> Style {
        on(style, self.dialog_surface)
    }
}

fn on(style: Style, background: Color) -> Style {
    if background == Color::Reset {
        style
    } else {
        style.bg(background)
    }
}

/// The theme for a configured name and the `NO_COLOR` environment variable.
///
/// `NO_COLOR` set to any non-empty value wins over the configured name
/// (https://no-color.org).
pub fn resolve(name: &str, no_color: Option<&std::ffi::OsStr>) -> Theme {
    if no_color.is_some_and(|value| !value.is_empty()) {
        mono()
    } else {
        from_name(name)
    }
}

/// Unknown names use the default theme.
pub fn from_name(name: &str) -> Theme {
    match name {
        "ops" => palette(OPS),
        "transparent" | "ops-transparent" => transparent(palette(OPS)),
        "catppuccin" => palette(CATPPUCCIN),
        "catppuccin-transparent" => transparent(palette(CATPPUCCIN)),
        "gruvbox" => palette(GRUVBOX),
        "gruvbox-transparent" => transparent(palette(GRUVBOX)),
        "mono" => mono(),
        _ => terminal(),
    }
}

/// The terminal's own foreground, background and 16 colors.
fn terminal() -> Theme {
    let bold = Modifier::BOLD;
    Theme {
        text: Style::default(),
        muted: Style::default().add_modifier(Modifier::DIM),
        accent: Style::default().fg(Color::Cyan).add_modifier(bold),
        selection: Style::default().add_modifier(Modifier::REVERSED | bold),
        success: Style::default().fg(Color::Green),
        error: Style::default().fg(Color::Red),
        warning: Style::default().fg(Color::Yellow),
        danger: Style::default().fg(Color::Red).add_modifier(bold),
        border: Style::default().add_modifier(Modifier::DIM),
        disabled: Style::default().add_modifier(Modifier::DIM),
        surface: Color::Reset,
        dialog_surface: Color::Reset,
    }
}

/// No colors at all: bold, dim and reverse video carry every role.
fn mono() -> Theme {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    Theme {
        accent: bold,
        success: Style::default(),
        error: bold,
        warning: bold,
        danger: bold,
        ..terminal()
    }
}

/// A fixed 24-bit palette that paints its own background.
#[derive(Clone, Copy)]
struct Palette {
    fg: (u8, u8, u8),
    fg_muted: (u8, u8, u8),
    disabled: (u8, u8, u8),
    bg: (u8, u8, u8),
    bg_dialog: (u8, u8, u8),
    accent: (u8, u8, u8),
    success: (u8, u8, u8),
    error: (u8, u8, u8),
    warning: (u8, u8, u8),
    border: (u8, u8, u8),
    selection_bg: (u8, u8, u8),
    selection_fg: (u8, u8, u8),
}

const OPS: Palette = Palette {
    fg: (250, 249, 245),
    fg_muted: (170, 166, 157),
    disabled: (119, 113, 104),
    bg: (20, 20, 19),
    bg_dialog: (39, 36, 31),
    accent: (240, 139, 101),
    success: (145, 199, 136),
    error: (255, 123, 114),
    warning: (230, 182, 115),
    border: (119, 113, 104),
    selection_bg: (168, 93, 70),
    selection_fg: (250, 249, 245),
};

const CATPPUCCIN: Palette = Palette {
    fg: (205, 214, 244),
    fg_muted: (166, 173, 200),
    disabled: (127, 132, 156),
    bg: (30, 30, 46),
    bg_dialog: (49, 50, 68),
    accent: (137, 180, 250),
    success: (166, 227, 161),
    error: (243, 139, 168),
    warning: (249, 226, 175),
    border: (88, 91, 112),
    selection_bg: (88, 91, 112),
    selection_fg: (205, 214, 244),
};

const GRUVBOX: Palette = Palette {
    fg: (235, 219, 178),
    fg_muted: (168, 153, 132),
    disabled: (146, 131, 116),
    bg: (29, 32, 33),
    bg_dialog: (40, 40, 40),
    accent: (131, 165, 152),
    success: (184, 187, 38),
    error: (255, 105, 84),
    warning: (250, 189, 47),
    border: (124, 111, 100),
    selection_bg: (87, 145, 148),
    selection_fg: (29, 32, 33),
};

fn rgb((red, green, blue): (u8, u8, u8)) -> Color {
    Color::Rgb(red, green, blue)
}

fn palette(palette: Palette) -> Theme {
    let fg = |color| Style::default().fg(rgb(color));
    let bold = Modifier::BOLD;
    Theme {
        text: fg(palette.fg),
        muted: fg(palette.fg_muted),
        accent: fg(palette.accent).add_modifier(bold),
        selection: fg(palette.selection_fg)
            .bg(rgb(palette.selection_bg))
            .add_modifier(bold),
        success: fg(palette.success),
        error: fg(palette.error),
        warning: fg(palette.warning),
        danger: fg(palette.error).add_modifier(bold),
        border: fg(palette.border),
        disabled: fg(palette.disabled),
        surface: rgb(palette.bg),
        dialog_surface: rgb(palette.bg_dialog),
    }
}

/// Keep a palette's colors but draw on the terminal's own background.
fn transparent(mut theme: Theme) -> Theme {
    theme.surface = Color::Reset;
    theme.dialog_surface = Color::Reset;
    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMED: [&str; 8] = [
        "terminal",
        "mono",
        "ops",
        "transparent",
        "catppuccin",
        "catppuccin-transparent",
        "gruvbox",
        "gruvbox-transparent",
    ];

    fn styles(theme: &Theme) -> [Style; 10] {
        [
            theme.text,
            theme.muted,
            theme.accent,
            theme.selection,
            theme.success,
            theme.error,
            theme.warning,
            theme.danger,
            theme.border,
            theme.disabled,
        ]
    }

    fn is_rgb(color: Option<Color>) -> bool {
        matches!(color, Some(Color::Rgb(..)))
    }

    #[test]
    fn default_theme_uses_only_the_terminals_own_colors_and_background() {
        let theme = from_name("terminal");

        assert_eq!(theme.surface, Color::Reset);
        assert_eq!(theme.dialog_surface, Color::Reset);
        assert_eq!(theme.text, Style::default());
        for style in styles(&theme) {
            assert!(!is_rgb(style.fg) && !is_rgb(style.bg), "{style:?}");
            assert_ne!(
                style.fg,
                Some(Color::White),
                "unreadable on a light terminal"
            );
            assert_ne!(
                style.fg,
                Some(Color::Black),
                "unreadable on a dark terminal"
            );
        }
        assert!(theme.selection.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn unset_empty_and_unknown_names_use_the_default_theme() {
        for name in [
            "",
            "default",
            "minimal",
            "nord",
            "solarized",
            "dark",
            "no-such-theme",
        ] {
            assert_eq!(from_name(name), from_name("terminal"), "{name}");
        }
    }

    #[test]
    fn no_color_forces_a_theme_without_any_color() {
        for name in NAMED {
            let theme = resolve(name, Some(std::ffi::OsStr::new("1")));
            assert_eq!(theme, from_name("mono"), "{name}");
        }
        for style in styles(&from_name("mono")) {
            assert_eq!((style.fg, style.bg), (None, None), "{style:?}");
        }
        assert_eq!(from_name("mono").surface, Color::Reset);
    }

    #[test]
    fn an_empty_no_color_value_does_not_disable_color() {
        assert_eq!(
            resolve("ops", Some(std::ffi::OsStr::new(""))),
            from_name("ops")
        );
        assert_eq!(resolve("ops", None), from_name("ops"));
    }

    #[test]
    fn every_theme_marks_selection_and_roles_without_relying_on_color_alone() {
        for name in NAMED {
            let theme = from_name(name);
            assert_ne!(theme.selection, theme.text, "{name}");
            assert_ne!(theme.muted, theme.text, "{name}");
            assert_ne!(theme.accent, theme.text, "{name}");
            assert_ne!(theme.danger, theme.text, "{name}");
        }
    }

    #[test]
    fn transparent_variants_keep_the_palette_on_the_terminal_background() {
        for (opaque, transparent) in [
            ("ops", "transparent"),
            ("ops", "ops-transparent"),
            ("catppuccin", "catppuccin-transparent"),
            ("gruvbox", "gruvbox-transparent"),
        ] {
            let (opaque, transparent) = (from_name(opaque), from_name(transparent));
            assert!(is_rgb(Some(opaque.surface)));
            assert_eq!(transparent.surface, Color::Reset);
            assert_eq!(transparent.dialog_surface, Color::Reset);
            assert_eq!(transparent.text, opaque.text);
            assert_eq!(transparent.selection, opaque.selection);
        }
    }

    #[test]
    fn palettes_keep_text_readable_on_their_own_surfaces() {
        for palette in [OPS, CATPPUCCIN, GRUVBOX] {
            for (foreground, background) in [
                (palette.fg, palette.bg),
                (palette.fg, palette.bg_dialog),
                (palette.fg_muted, palette.bg),
                (palette.fg_muted, palette.bg_dialog),
                (palette.selection_fg, palette.selection_bg),
                (palette.error, palette.bg_dialog),
            ] {
                assert!(
                    contrast(foreground, background) >= 4.5,
                    "{foreground:?} on {background:?}"
                );
            }
        }
    }

    fn contrast(foreground: (u8, u8, u8), background: (u8, u8, u8)) -> f64 {
        let (lighter, darker) = {
            let (first, second) = (luminance(foreground), luminance(background));
            if first >= second {
                (first, second)
            } else {
                (second, first)
            }
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn luminance((red, green, blue): (u8, u8, u8)) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
    }

    #[test]
    fn surface_helpers_never_paint_over_the_terminal_background() {
        let terminal = from_name("terminal");
        assert_eq!(terminal.on_surface(terminal.muted), terminal.muted);
        assert_eq!(terminal.on_dialog(terminal.text), terminal.text);

        let ops = from_name("ops");
        assert_eq!(ops.on_dialog(ops.text).bg, Some(ops.dialog_surface));
    }
}
