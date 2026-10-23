use leptos::ev;
use leptos::prelude::*;

#[derive(Default, Clone, Copy, PartialEq)]
pub enum ButtonFormat {
    #[default]
    Primary,
    Secondary,
    Ghost,
}

#[component]
pub fn Button(
    #[prop(into, optional)] on_click: Option<Callback<ev::MouseEvent>>,
    #[prop(into, optional)] disabled: Signal<bool>,
    #[prop(optional)] format: ButtonFormat,
    #[prop(into, optional)] class: String,
    #[prop(default = "button")] button_type: &'static str,
    children: Children,
) -> impl IntoView {
    let format_class = match format {
        ButtonFormat::Primary => "btn-primary",
        ButtonFormat::Secondary => "btn-secondary",
        ButtonFormat::Ghost => "btn-ghost",
    };

    view! {
        <button
            type=button_type
            class=format!("btn {format_class} {class}")
            disabled=move || disabled.get()
            on:click=move |ev| if let Some(cb) = on_click { cb.run(ev) }
        >
            {children()}
        </button>
    }
}
