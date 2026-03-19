use ratatui::style::Color;

/// Semantic color theme for the TUI.
///
/// Every TUI component reads colors from this struct instead of hardcoding them.
/// The `minimal` variant uses only basic ANSI colors (no 256-color or RGB needed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub foreground: Color,
    pub background: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub dimmed: Color,
    pub border: Color,
}

/// Resolve a theme name to a [`Theme`].
///
/// Returns the catppuccin theme (the default) for unrecognised names.
pub fn from_name(name: &str) -> Theme {
    match name {
        "catppuccin" => catppuccin(),
        "gruvbox" => gruvbox(),
        "minimal" => minimal(),
        _ => catppuccin(),
    }
}

fn catppuccin() -> Theme {
    // Catppuccin Mocha palette
    Theme {
        foreground: Color::Rgb(205, 214, 244), // Text
        background: Color::Rgb(30, 30, 46),    // Base
        accent: Color::Rgb(137, 180, 250),     // Blue
        success: Color::Rgb(166, 227, 161),    // Green
        error: Color::Rgb(243, 139, 168),      // Red
        warning: Color::Rgb(249, 226, 175),    // Yellow
        dimmed: Color::Rgb(127, 132, 156),     // Overlay0
        border: Color::Rgb(88, 91, 112),       // Surface2
    }
}

fn gruvbox() -> Theme {
    // Gruvbox Dark palette
    Theme {
        foreground: Color::Rgb(235, 219, 178), // fg
        background: Color::Rgb(40, 40, 40),    // bg
        accent: Color::Rgb(131, 165, 152),     // aqua
        success: Color::Rgb(184, 187, 38),     // green
        error: Color::Rgb(251, 73, 52),        // red
        warning: Color::Rgb(250, 189, 47),     // yellow
        dimmed: Color::Rgb(146, 131, 116),     // gray
        border: Color::Rgb(80, 73, 69),        // bg2
    }
}

fn minimal() -> Theme {
    // Basic ANSI colors only — works in any terminal
    Theme {
        foreground: Color::White,
        background: Color::Reset,
        accent: Color::Cyan,
        success: Color::Green,
        error: Color::Red,
        warning: Color::Yellow,
        dimmed: Color::DarkGray,
        border: Color::Gray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catppuccin_theme_has_expected_colors() {
        let theme = from_name("catppuccin");
        assert_eq!(theme.foreground, Color::Rgb(205, 214, 244));
        assert_eq!(theme.background, Color::Rgb(30, 30, 46));
        assert_eq!(theme.accent, Color::Rgb(137, 180, 250));
        assert_eq!(theme.success, Color::Rgb(166, 227, 161));
        assert_eq!(theme.error, Color::Rgb(243, 139, 168));
        assert_eq!(theme.warning, Color::Rgb(249, 226, 175));
        assert_eq!(theme.dimmed, Color::Rgb(127, 132, 156));
        assert_eq!(theme.border, Color::Rgb(88, 91, 112));
    }

    #[test]
    fn gruvbox_theme_has_expected_colors() {
        let theme = from_name("gruvbox");
        assert_eq!(theme.foreground, Color::Rgb(235, 219, 178));
        assert_eq!(theme.background, Color::Rgb(40, 40, 40));
        assert_eq!(theme.accent, Color::Rgb(131, 165, 152));
        assert_eq!(theme.success, Color::Rgb(184, 187, 38));
        assert_eq!(theme.error, Color::Rgb(251, 73, 52));
        assert_eq!(theme.warning, Color::Rgb(250, 189, 47));
        assert_eq!(theme.dimmed, Color::Rgb(146, 131, 116));
        assert_eq!(theme.border, Color::Rgb(80, 73, 69));
    }

    #[test]
    fn gruvbox_differs_from_catppuccin() {
        let cat = from_name("catppuccin");
        let grv = from_name("gruvbox");
        assert_ne!(cat, grv, "gruvbox and catppuccin should be different themes");
    }

    #[test]
    fn minimal_theme_uses_only_basic_ansi_colors() {
        let theme = from_name("minimal");
        let colors = [
            theme.foreground,
            theme.background,
            theme.accent,
            theme.success,
            theme.error,
            theme.warning,
            theme.dimmed,
            theme.border,
        ];
        for color in &colors {
            match color {
                Color::Rgb(_, _, _) => panic!("minimal theme must not use Rgb colors, found {color:?}"),
                Color::Indexed(_) => panic!("minimal theme must not use indexed colors, found {color:?}"),
                _ => {} // basic ANSI is fine
            }
        }
    }

    #[test]
    fn minimal_theme_has_expected_values() {
        let theme = from_name("minimal");
        assert_eq!(theme.foreground, Color::White);
        assert_eq!(theme.background, Color::Reset);
        assert_eq!(theme.accent, Color::Cyan);
        assert_eq!(theme.success, Color::Green);
        assert_eq!(theme.error, Color::Red);
        assert_eq!(theme.warning, Color::Yellow);
        assert_eq!(theme.dimmed, Color::DarkGray);
        assert_eq!(theme.border, Color::Gray);
    }

    #[test]
    fn minimal_differs_from_catppuccin() {
        let cat = from_name("catppuccin");
        let min = from_name("minimal");
        assert_ne!(cat, min, "minimal and catppuccin should be different themes");
    }

    #[test]
    fn theme_struct_has_all_semantic_fields() {
        let theme = from_name("catppuccin");
        // Verify each field is accessible and distinct
        let colors = [
            theme.foreground,
            theme.background,
            theme.accent,
            theme.success,
            theme.error,
            theme.warning,
            theme.dimmed,
            theme.border,
        ];
        // All 8 semantic colors should be distinct
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "colors at index {i} and {j} should differ");
                }
            }
        }
    }
}
