use leptos::prelude::*;

// Drawn icons in one stroke weight, sized by the surrounding text.

#[component]
pub fn ArrowLeft() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M13 8H3.5M7.5 4L3.5 8l4 4"/>
        </svg>
    }
}

#[component]
pub fn ArrowRight() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M3 8h9.5M8.5 4l4 4-4 4"/>
        </svg>
    }
}

#[component]
pub fn Chevron() -> impl IntoView {
    view! {
        <svg class="icon chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6 3.5L10.5 8 6 12.5"/>
        </svg>
    }
}
