//! The brand, after the IMF's seal: a round sticker of holographic metal
//! with the seal struck on it in steel, the whole seal heading the
//! directory and the shield alone in the header. The art is a file in
//! public/brand (see scripts/brand); .brand-sticker in style/main.css lays
//! the metal and the struck art.

use leptos::prelude::*;

/// The layers every sticker is made of, bottom to top: chrome, colour laid
/// into it, a glint, grain, the art struck in steel (a shade away from the
/// light, a lit lip toward it, the steel, the colour in it), and the
/// bevelled edge.
#[component]
fn Layers() -> impl IntoView {
    view! {
        <span class="metal"></span>
        <span class="holo"></span>
        <span class="glint"></span>
        <span class="grain"></span>
        <span class="art shade"></span>
        <span class="art lip"></span>
        <span class="art steel"></span>
        <span class="art tint"></span>
        <span class="bevel"></span>
    }
}

/// The header mark: the sticker with the shield alone, the legend being
/// unreadable at this size. Decoration beside the name, so hidden from
/// screen readers; the light sweeps across it once on hover or focus of the
/// link.
#[component]
pub fn BrandSticker() -> impl IntoView {
    view! {
        <span class="brand-sticker" aria-hidden="true">
            <Layers/>
        </span>
    }
}

/// The directory's masthead, one mark under one light: the sticker at full
/// size with the whole seal struck on it, and the name under it (the
/// children, the page's h1) struck in the seal's steel. The light sweeps in
/// once as the page arrives and settles at the hero light. After that it
/// follows the pointer anywhere over the mark: the sticker tilts, the light
/// moves across the metal and along the name, and the struck edges turn to
/// face it, then it glides home on leave.
#[component]
pub fn Masthead(children: Children) -> impl IntoView {
    let sticker = NodeRef::<leptos::html::Span>::new();
    let name = NodeRef::<leptos::html::H1>::new();
    // The pointer against the sticker (clamped to it) and across the name,
    // as fractions of their boxes; None when away.
    let pointer = RwSignal::new(None::<((f64, f64), f64)>);

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let (Some(sticker), Some(name)) = (sticker.get(), name.get()) else {
            return;
        };
        // Layout boxes, which the tilt doesn't move. Nothing above the
        // masthead is positioned, so their offsets are the page's.
        let fraction = |at: i32, start: i32, size: i32| {
            (f64::from(at - start) / f64::from(size.max(1))).clamp(0.0, 1.0)
        };
        let at = (
            fraction(ev.page_x(), sticker.offset_left(), sticker.offset_width()),
            fraction(ev.page_y(), sticker.offset_top(), sticker.offset_height()),
        );
        let along = fraction(ev.page_x(), name.offset_left(), name.offset_width());
        pointer.set(Some((at, along)));
    };

    let style = move || {
        pointer
            .get()
            .map(|(at, along)| format!("{}; --name-x: {:.1}%", light(at), 8.0 + along * 84.0))
            .unwrap_or_default()
    };

    view! {
        <div
            class="masthead"
            class:is-live=move || pointer.get().is_some()
            style=style
            on:pointermove=on_move
            on:pointerleave=move |_| pointer.set(None)
        >
            <span class="brand-sticker is-hero" node_ref=sticker aria-hidden="true">
                <Layers/>
            </span>
            <h1 class="masthead-title" node_ref=name>{children()}</h1>
        </div>
    }
}

/// A filled star cut from the tag's metal: steel with the colour laid in and
/// a glint, in the star's shape. Sized by `--star`; the empty star beside or
/// behind it stays a plain grey icon.
#[component]
pub fn FoilStar() -> impl IntoView {
    view! { <span class="foil-star" aria-hidden="true"></span> }
}

/// The custom properties that place the light for a pointer at (x, y): the
/// glint and the metal's bands follow it, the struck edges' lit side faces
/// it, and the sticker tilts toward it.
fn light((x, y): (f64, f64)) -> String {
    let (dx, dy) = (x - 0.5, y - 0.5);
    let n = dx.hypot(dy);
    // Straight overhead reads as lit from above.
    let (lx, ly) = if n < 0.08 {
        (0.0, -1.0)
    } else {
        (dx / n, dy / n)
    };
    format!(
        "--metal-px: {:.1}%; --metal-py: {:.1}%; --metal-bx: {:.1}%; --metal-by: {:.1}%; \
         --metal-lx: {lx:.3}; --metal-ly: {ly:.3}; --metal-rx: {:.2}deg; --metal-ry: {:.2}deg",
        x * 100.0,
        y * 100.0,
        8.0 + x * 84.0,
        8.0 + y * 84.0,
        (0.5 - y) * 16.0,
        (x - 0.5) * 20.0,
    )
}

#[cfg(test)]
mod tests {
    use super::light;

    #[test]
    fn the_light_faces_the_pointer_and_overhead_reads_as_above() {
        let corner = light((0.0, 0.0));
        assert!(corner.contains("--metal-px: 0.0%") && corner.contains("--metal-bx: 8.0%"));
        assert!(corner.contains("--metal-lx: -0.707") && corner.contains("--metal-ly: -0.707"));
        assert!(corner.contains("--metal-rx: 8.00deg") && corner.contains("--metal-ry: -10.00deg"));
        let centre = light((0.5, 0.52));
        assert!(centre.contains("--metal-lx: 0.000") && centre.contains("--metal-ly: -1.000"));
    }
}
