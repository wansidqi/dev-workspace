//! The app's motion system (CLAUDE.md: about 120 to 200 ms, ease-out, nothing
//! bouncy, and a reduced-motion setting).
//!
//! Three kinds of movement, all drawn from the tokens below:
//!
//! - **State springs** ([`fade`], [`phase`]): an element that flips between two
//!   states (hover, selected, on/off) glides there and keeps its velocity if
//!   interrupted, so rapid toggling never snaps or stacks. GPUI styles a hover
//!   instantly, so these are keyed on an element id instead.
//! - **Entrances** ([`enter`], [`enter_after`]): a one-shot fade plus a few
//!   pixels of rise, ease-out, for content that has just appeared (a view, a
//!   popover, a toast). Opacity and offset only: nothing here reflows layout.
//! - **Exits** are deliberately absent for overlays: they close at once so an
//!   invisible element never swallows input. The toast is the one exception
//!   and drops its hit area before it fades.
//!
//! Everything runs through GPUI's animation elements, which honour
//! [`gpui::App::reduce_motion`] by rendering the end state with no frames.

use std::time::Duration;

use gpui::{
    px, Animation, AnimationExt, ElementId, Hsla, IntoElement, SpringAnimation, SpringConfig,
    Styled,
};

/// Press and hover feedback.
pub const MICRO: Duration = Duration::from_millis(140);
/// Small components: tooltips, popovers, a toast leaving.
pub const FAST: Duration = Duration::from_millis(180);
/// Standard transitions: switching views, the palette, the toast arriving.
pub const STANDARD: Duration = Duration::from_millis(240);
/// Larger layout changes: a panel opening.
pub const LAYOUT: Duration = Duration::from_millis(320);
/// Gap between the starts of items in a staggered group.
pub const STAGGER: Duration = Duration::from_millis(45);
/// Most steps a staggered group may wait: later items share the last slot so a
/// long list never feels slow.
pub const STAGGER_MAX_STEPS: usize = 4;
/// How far, in pixels, an entrance travels. Small on purpose: it implies
/// direction without making content swim.
pub const RISE: f32 = 6.0;

/// Ease-out (cubic): fast start, soft landing. For things arriving.
pub fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Ease-in (quadratic): gentle start, quick finish. For things leaving.
pub fn ease_in(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t
}

/// Ease-in-out (cubic): for a change between two resting states.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

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

/// Drives `el` from a spring that sits at 0 while `on` is false and at 1
/// while it is true, calling `style` with the current position (clamped to
/// 0..=1) every frame. For anything with two resting states: a switch thumb,
/// a check mark, a tab underline. `id` must be unique and stable.
pub fn phase<E>(
    el: E,
    id: impl Into<ElementId>,
    on: bool,
    style: impl FnOnce(E, f32) -> E + 'static,
) -> impl IntoElement
where
    E: IntoElement + 'static,
{
    el.with_spring(id, SpringAnimation::new(TRANSITION).to(on), move |el, p| {
        style(el, progress(p.0))
    })
}

/// Like [`phase`], but the spring chases any `target` in 0..=1 (a share of a
/// bar, a fill level). A new element starts at its target, so only a *change*
/// of target moves; mounting or re-rendering never replays it. That keeps data
/// honest: movement always means the value changed.
pub fn follow<E>(
    el: E,
    id: impl Into<ElementId>,
    target: f32,
    style: impl FnOnce(E, f32) -> E + 'static,
) -> impl IntoElement
where
    E: IntoElement + 'static,
{
    el.with_spring(
        id,
        SpringAnimation::new(TRANSITION).to(target.clamp(0.0, 1.0)),
        move |el, p| style(el, progress(p)),
    )
}

/// Fades `el` in while it rises [`RISE`] pixels into place, once, when an
/// element with this `id` first appears. Re-rendering with the same `id` does
/// not replay it; a new `id` (a different view, a new toast) does. The offset
/// is relative positioning, so nothing around it reflows.
pub fn enter<E>(el: E, id: impl Into<ElementId>, duration: Duration) -> impl IntoElement
where
    E: Styled + IntoElement + 'static,
{
    enter_by(el, id, duration, RISE)
}

/// [`enter`] with an explicit travel distance (0 for a pure fade).
pub fn enter_by<E>(
    el: E,
    id: impl Into<ElementId>,
    duration: Duration,
    rise: f32,
) -> impl IntoElement
where
    E: Styled + IntoElement + 'static,
{
    el.with_animation(
        id,
        Animation::new(duration).with_easing(ease_out),
        move |el, t| entrance_style(el, t, rise),
    )
}

/// [`enter`] for the `index`th item of a short group: waits `index` steps
/// (capped at [`STAGGER_MAX_STEPS`]) fully transparent, then enters. Only for
/// a handful of items that appear together.
pub fn enter_after<E>(
    el: E,
    id: impl Into<ElementId>,
    index: usize,
    duration: Duration,
) -> impl IntoElement
where
    E: Styled + IntoElement + 'static,
{
    let steps = index.min(STAGGER_MAX_STEPS) as u32;
    // A zero-length wait is not allowed, so the first item waits 1 ms.
    let wait = Animation::new((STAGGER * steps).max(Duration::from_millis(1)));
    let run = Animation::new(duration).with_easing(ease_out);
    el.with_animations(id, vec![wait, run], move |el, step, t| {
        if step == 0 {
            el.opacity(0.0)
        } else {
            entrance_style(el, t, RISE)
        }
    })
}

/// The reverse of [`enter`] for the one thing that fades out (the toast):
/// ease-in, fading while sinking half the rise distance. The caller removes
/// the element when `duration` is up and should stop it taking input first.
pub fn leave<E>(el: E, id: impl Into<ElementId>, duration: Duration) -> impl IntoElement
where
    E: Styled + IntoElement + 'static,
{
    el.with_animation(
        id,
        Animation::new(duration).with_easing(ease_in),
        |el, t| {
            let t = t.clamp(0.0, 1.0);
            el.opacity(1.0 - t).relative().top(px(RISE * 0.5 * t))
        },
    )
}

/// Opacity and offset for an entrance at progress `t` (0 to 1). At rest it
/// adds nothing, so a finished entrance leaves the element as authored.
fn entrance_style<E: Styled>(el: E, t: f32, rise: f32) -> E {
    if t >= 1.0 {
        return el;
    }
    let t = t.clamp(0.0, 1.0);
    let el = el.opacity(t);
    // A pure fade leaves positioning alone, so it is safe on absolutely
    // positioned overlays.
    if rise == 0.0 {
        el
    } else {
        el.relative().top(px(rise * (1.0 - t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::hsla;

    #[test]
    fn easings_run_from_zero_to_one_and_stay_in_range() {
        for f in [ease_out, ease_in, ease_in_out] {
            assert_eq!(f(0.0), 0.0);
            assert!((f(1.0) - 1.0).abs() < 1e-6);
            assert_eq!(f(-1.0), 0.0);
            assert!((f(2.0) - 1.0).abs() < 1e-6);
        }
        // Out is front-loaded, in is back-loaded, in-out is symmetric.
        assert!(ease_out(0.3) > 0.3);
        assert!(ease_in(0.3) < 0.3);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn durations_follow_the_documented_ranges() {
        assert!((100..=180).contains(&MICRO.as_millis()));
        assert!((150..=250).contains(&FAST.as_millis()));
        assert!((200..=350).contains(&STANDARD.as_millis()));
        assert!((300..=450).contains(&LAYOUT.as_millis()));
        assert!(STAGGER * STAGGER_MAX_STEPS as u32 <= Duration::from_millis(250));
    }

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
