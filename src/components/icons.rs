use leptos::prelude::*;

// Drawn icons in one stroke weight, sized by the surrounding text.

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

#[component]
pub fn Copy() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/>
            <path d="M10.5 5.5V4A1.5 1.5 0 0 0 9 2.5H4A1.5 1.5 0 0 0 2.5 4v5A1.5 1.5 0 0 0 4 10.5h1.5"/>
        </svg>
    }
}

#[component]
pub fn Check() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M3 8.5l3.25 3.25L13 5"/>
        </svg>
    }
}

/// The check's opposite, for something not offered: a cross reads as "no".
#[component]
pub fn Cross() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" aria-hidden="true">
            <path d="M4.75 4.75l6.5 6.5M11.25 4.75l-6.5 6.5"/>
        </svg>
    }
}

/// A question mark in a ring, for "what does this mean?".
#[component]
pub fn Help() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="6.25"/>
            <path d="M6.25 6.25a1.75 1.75 0 1 1 2.5 1.6c-.45.2-.75.6-.75 1.1v.3"/>
            <path d="M8 11.25h.01"/>
        </svg>
    }
}

#[component]
pub fn Star() -> impl IntoView {
    view! {
        <svg class="icon star" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <path d="M8 1.25l2.06 4.18 4.61.67-3.34 3.25.79 4.59L8 11.77l-4.12 2.17.79-4.59L1.33 6.1l4.61-.67z"/>
        </svg>
    }
}

#[component]
pub fn User() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="8" cy="5.5" r="2.75"/>
            <path d="M2.75 14a5.25 5.25 0 0 1 10.5 0"/>
        </svg>
    }
}
