//! The IMF tag, the brand: an oval sticker of holographic metal, IMF engraved
//! in the header and struck in steel at full size beside the directory's
//! head. Both sizes are built from the same layers as the files in
//! public/brand (see scripts/brand) and styled by .brand-tag in
//! style/main.css.

use leptos::prelude::*;
use leptos::web_sys::HtmlElement;

/// The layers every tag is made of, bottom to top: chrome, colour laid into
/// it, a glint, grain, and the bevelled edge.
#[component]
fn Layers() -> impl IntoView {
    view! {
        <span class="brand-tag-layer metal"></span>
        <span class="brand-tag-layer holo"></span>
        <span class="brand-tag-layer glint"></span>
        <span class="brand-tag-layer grain"></span>
        <span class="brand-tag-layer bevel"></span>
    }
}

/// The header mark: IMF engraved solid, the size where shaded steel would
/// melt into the silver. Decoration beside the name, so hidden from screen
/// readers; the light sweeps across it once on hover of the link.
#[component]
pub fn BrandTag() -> impl IntoView {
    view! {
        <span class="brand-tag" aria-hidden="true">
            <Layers/>
            <span class="brand-tag-imf">"IMF"</span>
        </span>
    }
}

/// The tag at full size, set like a sticker pressed onto the directory's
/// head: IMF struck in raised steel and the name pressed along the bottom
/// (on phones IMF alone, the name being too small to read). The light sweeps
/// in once as the page arrives and settles at the angle the files are
/// rendered at. After that it follows the pointer over the tag: the sticker
/// tilts, the light moves across the metal and the struck edges turn to face
/// it, then it glides home on leave. The arrival waits for hydration
/// (`is-hydrated`), so its frames don't compete with the page coming alive.
#[component]
pub fn HeroTag() -> impl IntoView {
    // The pointer over the tag, as fractions of its box; None when away.
    let pointer = RwSignal::new(None::<(f64, f64)>);
    let hydrated = RwSignal::new(false);
    Effect::new(move |_| request_animation_frame(move || hydrated.set(true)));

    let on_move = move |ev: leptos::ev::PointerEvent| {
        // A finger starting a scroll isn't pointing at the tag.
        if ev.pointer_type() == "touch" {
            return;
        }
        // The layers take no pointer, so the target is the tag itself and
        // the offsets are against its own box, which the tilt doesn't move.
        let tag = event_target::<HtmlElement>(&ev);
        let (w, h) = (
            f64::from(tag.client_width()),
            f64::from(tag.client_height()),
        );
        if w > 0.0 && h > 0.0 {
            let x = (f64::from(ev.offset_x()) / w).clamp(0.0, 1.0);
            let y = (f64::from(ev.offset_y()) / h).clamp(0.0, 1.0);
            pointer.set(Some((x, y)));
        }
    };

    let style = move || pointer.get().map(light).unwrap_or_default();

    view! {
        <span
            class="brand-tag is-hero"
            class:is-live=move || pointer.get().is_some()
            class:is-hydrated=move || hydrated.get()
            style=style
            aria-hidden="true"
            on:pointermove=on_move
            on:pointerleave=move |_| pointer.set(None)
        >
            <Layers/>
            <svg class="brand-tag-arc" viewBox="0 0 100 72">
                <path id="hero-tag-arc" d="M 10 37 A 40 28 0 0 0 90 37" fill="none"></path>
                <text class="hi">
                    <textPath href="#hero-tag-arc" startOffset="50%" text-anchor="middle">"INTERNET MINT FINDER"</textPath>
                </text>
                <text class="lo">
                    <textPath href="#hero-tag-arc" startOffset="50%" text-anchor="middle">"INTERNET MINT FINDER"</textPath>
                </text>
            </svg>
            <span class="brand-tag-face">
                <span class="brand-tag-struck">
                    <span class="lo">"IMF"</span>
                    <span class="hi">"IMF"</span>
                    <span class="steel">"IMF"</span>
                    <span class="tint">"IMF"</span>
                </span>
            </span>
        </span>
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
/// glint and the metal's bands follow it, the letters' lit edge faces it,
/// and the sticker tilts toward it.
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
        "--tag-px: {:.1}%; --tag-py: {:.1}%; --tag-bx: {:.1}%; --tag-by: {:.1}%; \
         --tag-lx: {lx:.3}; --tag-ly: {ly:.3}; --tag-rx: {:.2}deg; --tag-ry: {:.2}deg",
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
        assert!(corner.contains("--tag-px: 0.0%") && corner.contains("--tag-bx: 8.0%"));
        assert!(corner.contains("--tag-lx: -0.707") && corner.contains("--tag-ly: -0.707"));
        assert!(corner.contains("--tag-rx: 8.00deg") && corner.contains("--tag-ry: -10.00deg"));
        let centre = light((0.5, 0.52));
        assert!(centre.contains("--tag-lx: 0.000") && centre.contains("--tag-ly: -1.000"));
    }
}
