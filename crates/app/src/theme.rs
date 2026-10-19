//! Central theme tokens: colour, shape, spacing and type.
//! Views must read colors/radii/spacing from here — never hardcode them.
//!
//! Visual identity ("graphite and phosphor"): cool graphite surfaces, one
//! phosphor-lime accent used for selection, focus, progress and primary
//! actions, and muted status colours. Geist carries the interface; Geist Mono
//! carries anything that is data (hashes, counts, dates, code).

use gpui::{hsla, px, App, Hsla, Pixels, Styled};
use std::borrow::Cow;

/// The interface typeface (bundled, see [`install_fonts`]). Falls back to
/// GPUI's default stack if the bundled data ever fails to load.
pub const UI_FONT: &str = "Geist";

/// Data typeface: hashes, counts, dates, code, note source. Bundled.
pub const CODE_FONT: &str = "Geist Mono";

/// The terminal keeps a system monospace face: terminal programs draw box and
/// powerline glyphs that a UI font does not cover. Per-OS because GPUI does
/// not ship fonts; each is present on a stock install of that OS.
#[cfg(target_os = "windows")]
pub const TERMINAL_FONT: &str = "Cascadia Mono";
#[cfg(target_os = "macos")]
pub const TERMINAL_FONT: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const TERMINAL_FONT: &str = "DejaVu Sans Mono";

/// Registers the bundled Geist and Geist Mono faces (three UI weights, two
/// code weights; OFL licence next to the files). Failure is logged, not fatal:
/// text then renders in the platform's default font.
pub fn install_fonts(cx: &App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../../../assets/fonts/Geist-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/Geist-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf")),
        Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/GeistMono-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/GeistMono-Medium.ttf")),
    ];
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("could not load the bundled fonts: {err}");
    }
}

/// Type scale in logical pixels. Six steps, three weights; anything outside
/// this list is a mistake.
pub const FS_MICRO: f32 = 11.0;
pub const FS_SMALL: f32 = 12.0;
pub const FS_BODY: f32 = 13.0;
pub const FS_PROSE: f32 = 14.0;
pub const FS_HEADING: f32 = 15.0;
pub const FS_TITLE: f32 = 20.0;
pub const FS_DISPLAY: f32 = 28.0;

/// Line height as a multiple of the font size for running text.
pub const LINE_HEIGHT: f32 = 1.45;

/// Named type steps for any styled element, so no view picks a raw size.
pub trait TypeScale: Styled + Sized {
    /// 11px: uppercase labels and fine print.
    fn t_micro(self) -> Self {
        self.text_size(px(FS_MICRO))
    }
    /// 12px: supporting text, metadata, chips.
    fn t_small(self) -> Self {
        self.text_size(px(FS_SMALL))
    }
    /// 13px: body text, controls, rows. The window default.
    fn t_body(self) -> Self {
        self.text_size(px(FS_BODY))
    }
    /// 14px: long-form reading (rendered notes).
    fn t_prose(self) -> Self {
        self.text_size(px(FS_PROSE))
    }
    /// 15px: section headings.
    fn t_heading(self) -> Self {
        self.text_size(px(FS_HEADING))
            .font_weight(gpui::FontWeight::SEMIBOLD)
    }
    /// 20px: page titles.
    fn t_title(self) -> Self {
        self.text_size(px(FS_TITLE))
            .font_weight(gpui::FontWeight::SEMIBOLD)
    }
    /// 28px: the one hero heading on a screen.
    fn t_display(self) -> Self {
        self.text_size(px(FS_DISPLAY))
            .font_weight(gpui::FontWeight::SEMIBOLD)
    }
    /// Geist Mono for data: hashes, counts, dates, code.
    fn t_code(self) -> Self {
        self.font_family(CODE_FONT)
    }
    /// Medium weight for emphasis inside a row (titles, active labels).
    fn t_strong(self) -> Self {
        self.font_weight(gpui::FontWeight::MEDIUM)
    }
}

impl<T: Styled + Sized> TypeScale for T {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassTheme {
    /// True for the light palette; a few things (terminal colours, tag chip
    /// lightness) adapt to it.
    pub is_light: bool,

    // Surfaces, back to front: window tint, chrome (sidebar), canvas (content
    // area), raised (cards, popovers). `surface_sunken` is for wells: inputs,
    // code blocks, logs.
    pub window_tint: Hsla,
    /// Dims the app behind a modal overlay (command palette).
    pub scrim: Hsla,
    pub surface: Hsla,
    pub surface_raised: Hsla,
    pub surface_solid: Hsla,
    pub surface_sunken: Hsla,
    /// Row hover wash, drawn over any surface.
    pub surface_hover: Hsla,

    pub border_light: Hsla,
    pub border_subtle: Hsla,
    pub border_strong: Hsla,

    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    /// Placeholders, disabled text and fine print. Not for anything the user
    /// must read (3:1 on the canvas, not 4.5:1).
    pub text_muted: Hsla,
    pub accent: Hsla,
    /// Text and icons drawn on a solid accent fill.
    pub accent_fg: Hsla,
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
    pub space_5: Pixels,
    pub space_6: Pixels,
    pub space_8: Pixels,
}

/// Cool graphite hue shared by every dark neutral.
const DARK_HUE: f32 = 228.0 / 360.0;
const LIGHT_HUE: f32 = 224.0 / 360.0;

impl GlassTheme {
    /// Dark theme first, per CLAUDE.md.
    pub fn dark() -> Self {
        Self {
            is_light: false,
            window_tint: hsla(DARK_HUE, 0.16, 0.050, 0.82),
            scrim: hsla(DARK_HUE, 0.3, 0.02, 0.62),
            surface: hsla(DARK_HUE, 0.14, 0.072, 0.72),
            surface_raised: hsla(DARK_HUE, 0.12, 0.125, 0.94),
            surface_solid: hsla(DARK_HUE, 0.12, 0.092, 1.0),
            surface_sunken: hsla(DARK_HUE, 0.16, 0.064, 1.0),
            surface_hover: hsla(0.0, 0.0, 1.0, 0.05),

            border_light: hsla(0.0, 0.0, 1.0, 0.11),
            border_subtle: hsla(0.0, 0.0, 1.0, 0.065),
            border_strong: hsla(0.0, 0.0, 1.0, 0.20),

            text_primary: hsla(DARK_HUE, 0.10, 0.93, 1.0),
            text_secondary: hsla(DARK_HUE, 0.07, 0.66, 1.0),
            text_muted: hsla(DARK_HUE, 0.06, 0.52, 1.0),
            accent: hsla(74.0 / 360.0, 0.80, 0.60, 1.0),
            accent_fg: hsla(80.0 / 360.0, 0.50, 0.08, 1.0),
            success: hsla(160.0 / 360.0, 0.48, 0.52, 1.0),
            warning: hsla(38.0 / 360.0, 0.88, 0.60, 1.0),
            danger: hsla(3.0 / 360.0, 0.74, 0.65, 1.0),

            radius_sm: px(6.0),
            radius_md: px(8.0),
            radius_lg: px(12.0),

            space_1: px(4.0),
            space_2: px(8.0),
            space_3: px(12.0),
            space_4: px(16.0),
            space_5: px(20.0),
            space_6: px(24.0),
            space_8: px(32.0),
        }
    }

    /// Light theme, second to dark per CLAUDE.md.
    pub fn light() -> Self {
        Self {
            is_light: true,
            window_tint: hsla(LIGHT_HUE, 0.22, 0.945, 0.72),
            scrim: hsla(LIGHT_HUE, 0.3, 0.15, 0.32),
            surface: hsla(LIGHT_HUE, 0.20, 0.955, 0.70),
            surface_raised: hsla(0.0, 0.0, 1.0, 0.90),
            surface_solid: hsla(LIGHT_HUE, 0.30, 0.992, 1.0),
            surface_sunken: hsla(LIGHT_HUE, 0.22, 0.952, 1.0),
            surface_hover: hsla(LIGHT_HUE, 0.35, 0.18, 0.055),

            border_light: hsla(LIGHT_HUE, 0.25, 0.20, 0.15),
            border_subtle: hsla(LIGHT_HUE, 0.25, 0.20, 0.085),
            border_strong: hsla(LIGHT_HUE, 0.25, 0.20, 0.26),

            text_primary: hsla(LIGHT_HUE, 0.25, 0.11, 1.0),
            text_secondary: hsla(LIGHT_HUE, 0.10, 0.37, 1.0),
            text_muted: hsla(LIGHT_HUE, 0.08, 0.52, 1.0),
            accent: hsla(80.0 / 360.0, 0.85, 0.25, 1.0),
            accent_fg: hsla(70.0 / 360.0, 0.60, 0.97, 1.0),
            success: hsla(160.0 / 360.0, 0.60, 0.30, 1.0),
            warning: hsla(32.0 / 360.0, 0.95, 0.38, 1.0),
            danger: hsla(2.0 / 360.0, 0.70, 0.45, 1.0),

            radius_sm: px(6.0),
            radius_md: px(8.0),
            radius_lg: px(12.0),

            space_1: px(4.0),
            space_2: px(8.0),
            space_3: px(12.0),
            space_4: px(16.0),
            space_5: px(20.0),
            space_6: px(24.0),
            space_8: px(32.0),
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

/// Colours for code in rendered notes, per palette: keywords coral, strings
/// blue, numbers teal, function calls amber, comments muted.
pub fn syntax_color(theme: &GlassTheme, token: crate::highlight::Token) -> Hsla {
    use crate::highlight::Token;
    let l = if theme.is_light { 0.40 } else { 0.70 };
    match token {
        Token::Keyword => hsla(6.0 / 360.0, 0.72, l, 1.0),
        Token::String => hsla(208.0 / 360.0, 0.78, l, 1.0),
        Token::Number => hsla(176.0 / 360.0, 0.55, l - 0.03, 1.0),
        Token::Function => hsla(32.0 / 360.0, 0.85, l - 0.02, 1.0),
        Token::Comment => theme.text_muted,
    }
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
            // Muted text is placeholders and fine print only: 3:1.
            assert!(contrast(theme.text_muted, theme.surface_solid) >= 3.0);
            assert!(contrast(theme.text_muted, theme.surface_sunken) >= 3.0);
            // Primary buttons draw label text on a solid accent fill.
            assert!(contrast(theme.accent_fg, theme.accent) >= 4.5);
            // Status colours carry text (chips) on the canvas and the wells.
            for status in [theme.success, theme.warning, theme.danger] {
                assert!(contrast(status, theme.surface_solid) >= 3.0);
                assert!(contrast(status, theme.surface_sunken) >= 3.0);
            }
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
