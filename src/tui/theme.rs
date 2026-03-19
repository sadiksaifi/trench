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
