//! Central glass theme tokens (CLAUDE.md "UI design: modern glass concept").
//! Views must read colors/radii/spacing from here — never hardcode them.

use gpui::{hsla, px, Hsla, Pixels};

/// The whole UI is set in the monospace family (user preference). Per-OS
/// because GPUI does not ship fonts;
/// each is present on a stock install of that OS.
#[cfg(target_os = "windows")]
pub const MONO_FONT: &str = "Cascadia Mono";
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const MONO_FONT: &str = "DejaVu Sans Mono";

/// UI text uses the same monospace family.
pub const UI_FONT: &str = MONO_FONT;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassTheme {
    /// True for the light palette; a few things (terminal colours, tag chip
    /// lightness) adapt to it.
    pub is_light: bool,

    pub window_tint: Hsla,
    pub surface: Hsla,
    pub surface_raised: Hsla,
    pub surface_solid: Hsla,

    pub border_light: Hsla,
    pub border_subtle: Hsla,

    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub accent: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,

    pub radius_sm: Pixels,
    pub radius_md: Pixels,
    pub radius_lg: Pixels,

    pub space_1: Pixels,
    pub space_2: Pixels,
    pub space_3: Pixels,
    pub space_4: Pixels,
}

impl GlassTheme {
    /// Dark theme first, per CLAUDE.md.
    pub fn dark() -> Self {
        Self {
            is_light: false,
            window_tint: hsla(0.0, 0.0, 0.04, 0.80),
            surface: hsla(0.0, 0.0, 0.08, 0.72),
            surface_raised: hsla(0.0, 0.0, 0.12, 0.88),
            surface_solid: hsla(0.0, 0.0, 0.07, 1.0),

            border_light: hsla(0.0, 0.0, 1.0, 0.10),
            border_subtle: hsla(0.0, 0.0, 1.0, 0.06),

            text_primary: hsla(0.0, 0.0, 0.92, 1.0),
            text_secondary: hsla(0.0, 0.0, 0.60, 1.0),
            accent: hsla(212.0 / 360.0, 0.85, 0.64, 1.0),
            success: hsla(142.0 / 360.0, 0.4, 0.5, 1.0),
            warning: hsla(38.0 / 360.0, 0.55, 0.55, 1.0),
            danger: hsla(0.0, 0.55, 0.58, 1.0),

            radius_sm: px(8.0),
            radius_md: px(12.0),
            radius_lg: px(16.0),

            space_1: px(4.0),
            space_2: px(8.0),
            space_3: px(12.0),
            space_4: px(16.0),
        }
    }

    /// Light theme, second to dark per CLAUDE.md.
    pub fn light() -> Self {
        Self {
            is_light: true,
            window_tint: hsla(220.0 / 360.0, 0.3, 0.96, 0.6),
            surface: hsla(220.0 / 360.0, 0.25, 0.97, 0.65),
            surface_raised: hsla(220.0 / 360.0, 0.3, 0.99, 0.82),
            surface_solid: hsla(220.0 / 360.0, 0.3, 0.98, 1.0),

            border_light: hsla(0.0, 0.0, 1.0, 0.6),
            border_subtle: hsla(220.0 / 360.0, 0.15, 0.2, 0.08),

            text_primary: hsla(220.0 / 360.0, 0.2, 0.12, 1.0),
            text_secondary: hsla(220.0 / 360.0, 0.1, 0.35, 1.0),
            accent: hsla(217.0 / 360.0, 0.8, 0.5, 1.0),
            success: hsla(142.0 / 360.0, 0.45, 0.38, 1.0),
            warning: hsla(38.0 / 360.0, 0.6, 0.45, 1.0),
            danger: hsla(0.0, 0.6, 0.48, 1.0),

            radius_sm: px(8.0),
            radius_md: px(12.0),
            radius_lg: px(16.0),

            space_1: px(4.0),
            space_2: px(8.0),
            space_3: px(12.0),
            space_4: px(16.0),
        }
    }
}

impl GlassTheme {
    /// The same palette with opaque surfaces, for "reduce transparency" and
    /// for systems where the window blur is unavailable. Content colours,
    /// borders and shape are untouched.
    pub fn solid(mut self) -> Self {
        self.window_tint.a = 1.0;
        self.surface.a = 1.0;
        self.surface_raised.a = 1.0;
        self
    }

    /// The theme for a settings choice: `mode` picks the palette (following
    /// `os_is_dark` for [`ThemeMode::System`]) and `reduce_transparency`
    /// makes the surfaces solid.
    pub fn resolve(mode: ThemeMode, os_is_dark: bool, reduce_transparency: bool) -> Self {
        let dark = match mode {
            ThemeMode::System => os_is_dark,
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
        };
        let theme = if dark { Self::dark() } else { Self::light() };
        if reduce_transparency {
            theme.solid()
        } else {
            theme
        }
    }
}

impl Default for GlassTheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// The theme every view draws with. Views copy it at the top of `render`
/// (see [`current`]), so changing it and refreshing the windows restyles the
/// whole app at runtime.
pub struct ActiveTheme(pub GlassTheme);

impl gpui::Global for ActiveTheme {}

/// The active theme (the dark default before one has been set).
pub fn current(cx: &gpui::App) -> GlassTheme {
    cx.try_global::<ActiveTheme>()
        .map_or_else(GlassTheme::default, |active| active.0)
}

/// A stable colour for a label (tag or repo name): the same text always gets
/// the same hue, so chips are recognisable at a glance. Saturation is fixed
/// and lightness follows the theme, so every hue stays readable on both the
/// dark and the light surfaces.
pub fn label_color(theme: &GlassTheme, label: &str) -> Hsla {
    // FNV-1a: tiny, deterministic across runs (unlike std's randomised hasher).
    let mut hash: u32 = 0x811c_9dc5;
    for byte in label.to_lowercase().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let lightness = if theme.is_light { 0.36 } else { 0.68 };
    hsla((hash % 360) as f32 / 360.0, 0.62, lightness, 1.0)
}

/// Darkens a terminal foreground colour enough to read on the light theme
/// (programs pick colours for dark backgrounds). Dark themes are unchanged.
pub fn terminal_fg(theme: &GlassTheme, color: Hsla) -> Hsla {
    if theme.is_light && color.l > 0.4 {
        Hsla { l: 0.4, ..color }
    } else {
        color
    }
}

/// Which palette the app uses. Dark is the default; the user can switch to
/// light or have it follow the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    System,
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    /// Name stored in `settings.txt`.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    /// Name shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "Follow system",
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }

    /// Reads a stored name; `None` for anything unknown.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" => Some(ThemeMode::System),
            "dark" => Some(ThemeMode::Dark),
            "light" => Some(ThemeMode::Light),
            _ => None,
        }
    }

    /// The mode a click moves to: System -> Dark -> Light -> System.
    pub fn cycle(self) -> Self {
        match self {
            ThemeMode::System => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::System,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_follows_the_os_only_in_system_mode() {
        assert!(!GlassTheme::resolve(ThemeMode::System, true, false).is_light);
        assert!(GlassTheme::resolve(ThemeMode::System, false, false).is_light);
        assert!(!GlassTheme::resolve(ThemeMode::Dark, false, false).is_light);
        assert!(GlassTheme::resolve(ThemeMode::Light, true, false).is_light);
        assert_eq!(
            GlassTheme::resolve(ThemeMode::Dark, false, false),
            GlassTheme::dark()
        );
    }

    #[test]
    fn reduced_transparency_makes_surfaces_opaque_and_changes_nothing_else() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let normal = GlassTheme::resolve(mode, true, false);
            let solid = GlassTheme::resolve(mode, true, true);
            assert!(normal.surface.a < 1.0, "the glass theme is translucent");
            assert_eq!(solid.window_tint.a, 1.0);
            assert_eq!(solid.surface.a, 1.0);
            assert_eq!(solid.surface_raised.a, 1.0);
            // Same palette otherwise.
            assert_eq!(solid.text_primary, normal.text_primary);
            assert_eq!(solid.accent, normal.accent);
            assert_eq!(solid.surface_solid, normal.surface_solid);
            assert_eq!(solid.is_light, normal.is_light);
        }
    }

    #[test]
    fn text_is_readable_against_the_solid_surface_in_both_themes() {
        // WCAG-style relative luminance contrast, on the opaque surface.
        fn luminance(c: Hsla) -> f32 {
            let rgb = c.to_rgb();
            let lin = |v: f32| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(rgb.r) + 0.7152 * lin(rgb.g) + 0.0722 * lin(rgb.b)
        }
        fn contrast(a: Hsla, b: Hsla) -> f32 {
            let (la, lb) = (luminance(a), luminance(b));
            (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
        }
        for theme in [GlassTheme::dark().solid(), GlassTheme::light().solid()] {
            assert!(contrast(theme.text_primary, theme.surface_solid) >= 7.0);
            // Secondary text is body text too: WCAG AA is 4.5:1.
            assert!(contrast(theme.text_secondary, theme.surface_solid) >= 4.5);
            assert!(contrast(theme.accent, theme.surface_solid) >= 3.0);
        }
    }

    #[test]
    fn tag_colours_follow_the_theme_and_stay_stable() {
        let dark = GlassTheme::dark();
        let light = GlassTheme::light();
        assert_eq!(label_color(&dark, "Bug"), label_color(&dark, "bug"));
        assert_eq!(label_color(&dark, "bug").h, label_color(&light, "bug").h);
        assert!(label_color(&dark, "bug").l > label_color(&light, "bug").l);
    }

    #[test]
    fn terminal_colours_are_only_darkened_on_the_light_theme() {
        let pale = hsla(0.1, 0.8, 0.9, 1.0);
        assert_eq!(terminal_fg(&GlassTheme::dark(), pale), pale);
        let adjusted = terminal_fg(&GlassTheme::light(), pale);
        assert!(adjusted.l <= 0.4);
        assert_eq!(adjusted.h, pale.h);
        let already_dark = hsla(0.5, 0.5, 0.2, 1.0);
        assert_eq!(
            terminal_fg(&GlassTheme::light(), already_dark),
            already_dark
        );
    }

    #[test]
    fn theme_mode_round_trips_and_cycles_through_all_three() {
        let mut mode = ThemeMode::System;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(mode);
            assert_eq!(ThemeMode::parse(mode.as_str()), Some(mode));
            mode = mode.cycle();
        }
        assert_eq!(seen, [ThemeMode::System, ThemeMode::Dark, ThemeMode::Light]);
        assert_eq!(mode, ThemeMode::System);
        assert_eq!(ThemeMode::parse(" DARK "), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::parse("blue"), None);
    }
}
