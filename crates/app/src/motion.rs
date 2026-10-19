//! Short transitions for hover and selection (CLAUDE.md: about 120 to 200 ms,
//! ease-out, nothing bouncy, and a reduced-motion setting).
//!
//! GPUI styles a hover instantly, so a row that should fade in is driven by a
//! spring keyed on its element id: when `active` flips, the spring glides to
//! its new target and keeps its velocity if interrupted. Springs honour
//! [`gpui::App::reduce_motion`] by themselves: with it set, the element jumps
//! straight to its end state and no frames are scheduled.

use gpui::{AnimationExt, ElementId, Hsla, IntoElement, SpringAnimation, SpringConfig, Styled};

/// Critically damped (no overshoot) and settles in roughly 150 to 200 ms.
pub const TRANSITION: SpringConfig = SpringConfig::new(1600.0, 80.0, 1.0);

/// How far through the transition a row is, 0 (inactive) to 1 (active),
/// clamped because a spring can overshoot its range slightly.
pub fn progress(phase: f32) -> f32 {
    phase.clamp(0.0, 1.0)
}

/// `color` scaled to `phase` of its own opacity: fully transparent at 0,
/// unchanged at 1.
pub fn faded(color: Hsla, phase: f32) -> Hsla {
    // `opacity` already multiplies the colour's own alpha.
    color.opacity(progress(phase))
}

/// Wraps `el` so its background and border fade in when `active` becomes true
/// and out when it becomes false. `id` must be unique and stable for the
/// element, because the spring's position is stored under it. Apply this last:
/// the wrapper is no longer a styled element.
pub fn fade<E>(
    el: E,
    id: impl Into<ElementId>,
    active: bool,
    bg: Hsla,
    border: Hsla,
) -> impl IntoElement
where
    E: Styled + IntoElement + 'static,
{
    el.with_spring(
        id,
        SpringAnimation::new(TRANSITION).to(active),
        move |el, phase| {
            el.bg(faded(bg, phase.0))
                .border_color(faded(border, phase.0))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::hsla;

    #[test]
    fn progress_is_clamped() {
        assert_eq!(progress(-0.2), 0.0);
        assert_eq!(progress(0.4), 0.4);
        assert_eq!(progress(1.07), 1.0);
    }

    #[test]
    fn fade_scales_opacity_and_keeps_the_colour() {
        let c = hsla(0.5, 0.6, 0.4, 0.8);
        assert_eq!(faded(c, 0.0).a, 0.0);
        assert!((faded(c, 0.5).a - 0.4).abs() < 1e-6);
        assert_eq!(faded(c, 1.0), c);
        let half = faded(c, 0.5);
        assert_eq!((half.h, half.s, half.l), (c.h, c.s, c.l));
    }

    #[test]
    fn the_transition_does_not_overshoot_and_settles_quickly() {
        let (_, damping) = TRANSITION.canonical();
        assert!(damping >= 1.0 - 1e-3, "must be critically damped or more");
        let settle = TRANSITION.settle_time(
            gpui::SpringState {
                position: 0.0,
                velocity: 0.0,
            },
            1.0,
            0.01,
        );
        let ms = settle.as_millis();
        assert!((60..=300).contains(&ms), "settles in {ms} ms");
    }
}
