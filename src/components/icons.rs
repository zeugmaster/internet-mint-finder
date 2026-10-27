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

#[component]
pub fn Sun() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="3"/>
            <path d="M8 1.5v1.25M8 13.25v1.25M1.5 8h1.25M13.25 8h1.25M3.4 3.4l.9.9M11.7 11.7l.9.9M3.4 12.6l.9-.9M11.7 4.3l.9-.9"/>
        </svg>
    }
}

#[component]
pub fn Moon() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M13.5 9.6A5.75 5.75 0 0 1 6.4 2.5a5.75 5.75 0 1 0 7.1 7.1z"/>
        </svg>
    }
}
