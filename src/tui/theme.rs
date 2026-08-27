use ratatui::style::{Color, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub fg: Color,
    pub fg_muted: Color,
    pub bg: Color,
    pub bg_elevated: Color,
    pub bg_panel: Color,
    pub control_bg: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub danger_bg: Color,
    pub danger_fg: Color,
    pub primary_bg: Color,
    pub primary_fg: Color,
    pub disabled_fg: Color,
    pub border: Color,
    pub border_active: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
}

impl Theme {
    pub fn with_bg(&self, style: Style, color: Color) -> Style {
        if color == Color::Reset {
            style
        } else {
            style.bg(color)
        }
    }
}

pub fn from_name(name: &str) -> Theme {
    match name {
        "ops" | "default" | "" => ops(),
        "transparent" | "ops-transparent" => transparent(ops()),
        "catppuccin" => catppuccin(),
        "catppuccin-transparent" | "nord-transparent" | "solarized-transparent" => {
            transparent(catppuccin())
        }
        "gruvbox" | "dark" => gruvbox(),
        "gruvbox-transparent" | "dark-transparent" => transparent(gruvbox()),
        "minimal" => minimal(),
        "nord" | "solarized" => catppuccin(),
        _ => ops(),
    }
}

fn ops() -> Theme {
    Theme {
        fg: Color::Rgb(250, 249, 245),
        fg_muted: Color::Rgb(170, 166, 157),
        bg: Color::Rgb(20, 20, 19),
        bg_elevated: Color::Rgb(39, 36, 31),
        bg_panel: Color::Rgb(28, 27, 24),
        control_bg: Color::Rgb(50, 46, 41),
        accent: Color::Rgb(240, 139, 101),
        accent_soft: Color::Rgb(168, 93, 70),
        success: Color::Rgb(145, 199, 136),
        error: Color::Rgb(255, 123, 114),
        warning: Color::Rgb(230, 182, 115),
        danger_bg: Color::Rgb(162, 59, 56),
        danger_fg: Color::Rgb(250, 249, 245),
        primary_bg: Color::Rgb(240, 139, 101),
        primary_fg: Color::Rgb(20, 20, 19),
        disabled_fg: Color::Rgb(119, 113, 104),
        border: Color::Rgb(119, 113, 104),
        border_active: Color::Rgb(113, 183, 255),
        selection_bg: Color::Rgb(168, 93, 70),
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
        control_bg: Color::Rgb(69, 71, 90),
        accent: Color::Rgb(137, 180, 250),
        accent_soft: Color::Rgb(69, 71, 90),
        success: Color::Rgb(166, 227, 161),
        error: Color::Rgb(243, 139, 168),
        warning: Color::Rgb(249, 226, 175),
        danger_bg: Color::Rgb(137, 70, 90),
        danger_fg: Color::Rgb(255, 255, 255),
        primary_bg: Color::Rgb(137, 180, 250),
        primary_fg: Color::Rgb(30, 30, 46),
        disabled_fg: Color::Rgb(127, 132, 156),
        border: Color::Rgb(88, 91, 112),
        border_active: Color::Rgb(137, 180, 250),
        selection_bg: Color::Rgb(88, 91, 112),
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
        control_bg: Color::Rgb(60, 56, 54),
        accent: Color::Rgb(131, 165, 152),
        accent_soft: Color::Rgb(69, 133, 136),
        success: Color::Rgb(184, 187, 38),
        error: Color::Rgb(251, 73, 52),
        warning: Color::Rgb(250, 189, 47),
        danger_bg: Color::Rgb(157, 0, 6),
        danger_fg: Color::Rgb(251, 241, 199),
        primary_bg: Color::Rgb(131, 165, 152),
        primary_fg: Color::Rgb(29, 32, 33),
        disabled_fg: Color::Rgb(146, 131, 116),
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
        control_bg: Color::Black,
        accent: Color::Cyan,
        accent_soft: Color::Blue,
        success: Color::Green,
        error: Color::Red,
        warning: Color::Yellow,
        danger_bg: Color::Red,
        danger_fg: Color::White,
        primary_bg: Color::Cyan,
        primary_fg: Color::Black,
        disabled_fg: Color::DarkGray,
        border: Color::Gray,
        border_active: Color::White,
        selection_bg: Color::Blue,
        selection_fg: Color::White,
    }
}

fn transparent(mut theme: Theme) -> Theme {
    theme.bg = Color::Reset;
    theme.bg_elevated = Color::Reset;
    theme.bg_panel = Color::Reset;
    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_theme_has_expected_anchor_colors() {
        let theme = from_name("ops");
        assert_eq!(theme.fg, Color::Rgb(250, 249, 245));
        assert_eq!(theme.bg, Color::Rgb(20, 20, 19));
        assert_eq!(theme.bg_panel, Color::Rgb(28, 27, 24));
        assert_eq!(theme.bg_elevated, Color::Rgb(39, 36, 31));
        assert_eq!(theme.accent, Color::Rgb(240, 139, 101));
        assert_eq!(theme.border, Color::Rgb(119, 113, 104));
        assert_eq!(theme.border_active, Color::Rgb(113, 183, 255));
        assert_eq!(theme.selection_bg, Color::Rgb(168, 93, 70));
        assert_eq!(theme.selection_fg, Color::Rgb(250, 249, 245));
    }

    #[test]
    fn every_theme_has_distinct_control_and_selection_surfaces() {
        for name in ["ops", "catppuccin", "gruvbox", "minimal"] {
            let theme = from_name(name);
            assert_ne!(
                theme.control_bg, theme.selection_bg,
                "{name} must distinguish idle and selected controls"
            );
        }
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
            theme.control_bg,
            theme.accent,
            theme.accent_soft,
            theme.success,
            theme.error,
            theme.warning,
            theme.danger_bg,
            theme.danger_fg,
            theme.primary_bg,
            theme.primary_fg,
            theme.disabled_fg,
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
    fn transparent_theme_keeps_ops_palette_but_resets_base_surfaces() {
        let theme = from_name("transparent");
        assert_eq!(theme.fg, Color::Rgb(250, 249, 245));
        assert_eq!(theme.accent, Color::Rgb(240, 139, 101));
        assert_eq!(theme.bg, Color::Reset);
        assert_eq!(theme.bg_elevated, Color::Reset);
        assert_eq!(theme.bg_panel, Color::Reset);
    }

    #[test]
    fn catppuccin_transparent_alias_resets_base_surfaces() {
        let theme = from_name("catppuccin-transparent");
        assert_eq!(theme.fg, Color::Rgb(205, 214, 244));
        assert_eq!(theme.accent, Color::Rgb(137, 180, 250));
        assert_eq!(theme.bg, Color::Reset);
        assert_eq!(theme.bg_elevated, Color::Reset);
        assert_eq!(theme.bg_panel, Color::Reset);
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
            theme.control_bg,
            theme.accent,
            theme.accent_soft,
            theme.success,
            theme.error,
            theme.warning,
            theme.danger_bg,
            theme.danger_fg,
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
