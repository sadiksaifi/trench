use ratatui::style::Color;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub fg: Color,
    pub fg_muted: Color,
    pub bg: Color,
    pub bg_elevated: Color,
    pub bg_panel: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub border: Color,
    pub border_active: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
}

pub fn from_name(name: &str) -> Theme {
    match name {
        "ops" | "default" | "" => ops(),
        "catppuccin" => catppuccin(),
        "gruvbox" | "dark" => gruvbox(),
        "minimal" => minimal(),
        "nord" | "solarized" => catppuccin(),
        _ => ops(),
    }
}

fn ops() -> Theme {
    Theme {
        fg: Color::Rgb(250, 249, 245),
        fg_muted: Color::Rgb(176, 174, 165),
        bg: Color::Rgb(20, 20, 19),
        bg_elevated: Color::Rgb(28, 27, 24),
        bg_panel: Color::Rgb(34, 33, 30),
        accent: Color::Rgb(217, 119, 87),
        accent_soft: Color::Rgb(88, 63, 55),
        success: Color::Rgb(120, 140, 93),
        error: Color::Rgb(201, 95, 77),
        warning: Color::Rgb(201, 154, 99),
        border: Color::Rgb(62, 58, 51),
        border_active: Color::Rgb(106, 155, 204),
        selection_bg: Color::Rgb(60, 44, 39),
        selection_fg: Color::Rgb(250, 249, 245),
    }
}

fn catppuccin() -> Theme {
    Theme {
        fg: Color::Rgb(205, 214, 244),
        fg_muted: Color::Rgb(127, 132, 156),
        bg: Color::Rgb(30, 30, 46),
        bg_elevated: Color::Rgb(49, 50, 68),
        bg_panel: Color::Rgb(24, 24, 37),
        accent: Color::Rgb(137, 180, 250),
        accent_soft: Color::Rgb(69, 71, 90),
        success: Color::Rgb(166, 227, 161),
        error: Color::Rgb(243, 139, 168),
        warning: Color::Rgb(249, 226, 175),
        border: Color::Rgb(88, 91, 112),
        border_active: Color::Rgb(137, 180, 250),
        selection_bg: Color::Rgb(69, 71, 90),
        selection_fg: Color::Rgb(205, 214, 244),
    }
}

fn gruvbox() -> Theme {
    Theme {
        fg: Color::Rgb(235, 219, 178),
        fg_muted: Color::Rgb(168, 153, 132),
        bg: Color::Rgb(29, 32, 33),
        bg_elevated: Color::Rgb(40, 40, 40),
        bg_panel: Color::Rgb(50, 48, 47),
        accent: Color::Rgb(131, 165, 152),
        accent_soft: Color::Rgb(69, 133, 136),
        success: Color::Rgb(184, 187, 38),
        error: Color::Rgb(251, 73, 52),
        warning: Color::Rgb(250, 189, 47),
        border: Color::Rgb(80, 73, 69),
        border_active: Color::Rgb(131, 165, 152),
        selection_bg: Color::Rgb(69, 133, 136),
        selection_fg: Color::Rgb(251, 241, 199),
    }
}

fn minimal() -> Theme {
    Theme {
        fg: Color::White,
        fg_muted: Color::DarkGray,
        bg: Color::Reset,
        bg_elevated: Color::Black,
        bg_panel: Color::Reset,
        accent: Color::Cyan,
        accent_soft: Color::Blue,
        success: Color::Green,
        error: Color::Red,
        warning: Color::Yellow,
        border: Color::Gray,
        border_active: Color::White,
        selection_bg: Color::Blue,
        selection_fg: Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_theme_has_expected_anchor_colors() {
        let theme = from_name("ops");
        assert_eq!(theme.fg, Color::Rgb(250, 249, 245));
        assert_eq!(theme.bg, Color::Rgb(20, 20, 19));
        assert_eq!(theme.accent, Color::Rgb(217, 119, 87));
        assert_eq!(theme.selection_fg, Color::Rgb(250, 249, 245));
    }

    #[test]
    fn catppuccin_theme_has_expected_colors() {
        let theme = from_name("catppuccin");
        assert_eq!(theme.fg, Color::Rgb(205, 214, 244));
        assert_eq!(theme.bg, Color::Rgb(30, 30, 46));
        assert_eq!(theme.accent, Color::Rgb(137, 180, 250));
        assert_eq!(theme.success, Color::Rgb(166, 227, 161));
        assert_eq!(theme.error, Color::Rgb(243, 139, 168));
        assert_eq!(theme.warning, Color::Rgb(249, 226, 175));
        assert_eq!(theme.fg_muted, Color::Rgb(127, 132, 156));
        assert_eq!(theme.border, Color::Rgb(88, 91, 112));
    }

    #[test]
    fn gruvbox_theme_has_expected_colors() {
        let theme = from_name("gruvbox");
        assert_eq!(theme.fg, Color::Rgb(235, 219, 178));
        assert_eq!(theme.bg, Color::Rgb(29, 32, 33));
        assert_eq!(theme.accent, Color::Rgb(131, 165, 152));
        assert_eq!(theme.success, Color::Rgb(184, 187, 38));
        assert_eq!(theme.error, Color::Rgb(251, 73, 52));
        assert_eq!(theme.warning, Color::Rgb(250, 189, 47));
        assert_eq!(theme.fg_muted, Color::Rgb(168, 153, 132));
        assert_eq!(theme.border, Color::Rgb(80, 73, 69));
    }

    #[test]
    fn gruvbox_differs_from_ops() {
        let ops = from_name("ops");
        let grv = from_name("gruvbox");
        assert_ne!(ops, grv, "gruvbox and ops should be different themes");
    }

    #[test]
    fn minimal_theme_uses_only_basic_ansi_colors() {
        let theme = from_name("minimal");
        let colors = [
            theme.fg,
            theme.fg_muted,
            theme.bg,
            theme.bg_elevated,
            theme.bg_panel,
            theme.accent,
            theme.accent_soft,
            theme.success,
            theme.error,
            theme.warning,
            theme.border,
            theme.border_active,
            theme.selection_bg,
            theme.selection_fg,
        ];
        for color in &colors {
            match color {
                Color::Rgb(_, _, _) => {
                    panic!("minimal theme must not use Rgb colors, found {color:?}")
                }
                Color::Indexed(_) => {
                    panic!("minimal theme must not use indexed colors, found {color:?}")
                }
                _ => {}
            }
        }
    }

    #[test]
    fn minimal_theme_has_expected_values() {
        let theme = from_name("minimal");
        assert_eq!(theme.fg, Color::White);
        assert_eq!(theme.fg_muted, Color::DarkGray);
        assert_eq!(theme.bg, Color::Reset);
        assert_eq!(theme.accent, Color::Cyan);
        assert_eq!(theme.success, Color::Green);
        assert_eq!(theme.error, Color::Red);
        assert_eq!(theme.warning, Color::Yellow);
        assert_eq!(theme.border, Color::Gray);
    }

    #[test]
    fn minimal_differs_from_ops() {
        let ops = from_name("ops");
        let min = from_name("minimal");
        assert_ne!(ops, min, "minimal and ops should be different themes");
    }

    #[test]
    fn invalid_theme_name_falls_back_to_ops() {
        let fallback = from_name("nonexistent");
        let ops = from_name("ops");
        assert_eq!(fallback, ops, "unknown theme should fall back to ops");
    }

    #[test]
    fn empty_theme_name_falls_back_to_ops() {
        let fallback = from_name("");
        let ops = from_name("ops");
        assert_eq!(fallback, ops);
    }

    #[test]
    fn theme_struct_has_all_semantic_fields() {
        let theme = from_name("ops");
        let colors = [
            theme.fg,
            theme.fg_muted,
            theme.bg,
            theme.bg_elevated,
            theme.bg_panel,
            theme.accent,
            theme.accent_soft,
            theme.success,
            theme.error,
            theme.warning,
            theme.border,
            theme.border_active,
            theme.selection_bg,
            theme.selection_fg,
        ];
        for (i, color) in colors.iter().enumerate() {
            assert_ne!(
                *color,
                Color::Reset,
                "color at index {i} should not be Color::Reset"
            );
        }
    }
}
