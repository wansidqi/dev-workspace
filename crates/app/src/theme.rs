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

#[derive(Debug, Clone, Copy)]
pub struct GlassTheme {
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

    /// Light theme, second to dark per CLAUDE.md. Not wired to a UI toggle
    /// yet (tracked in docs/progress.md) — kept ready for when it is.
    #[allow(dead_code)]
    pub fn light() -> Self {
        Self {
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

impl Default for GlassTheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// A stable colour for a label (tag or repo name): the same text always gets
/// the same hue, so chips are recognisable at a glance. Saturation and
/// lightness are fixed so every hue stays readable on the dark surfaces.
pub fn label_color(label: &str) -> Hsla {
    // FNV-1a: tiny, deterministic across runs (unlike std's randomised hasher).
    let mut hash: u32 = 0x811c_9dc5;
    for byte in label.to_lowercase().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hsla((hash % 360) as f32 / 360.0, 0.62, 0.68, 1.0)
}
