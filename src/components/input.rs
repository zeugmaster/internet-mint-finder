use leptos::prelude::*;

#[component]
pub fn Input(
    #[prop(into, optional)] label: Signal<String>,
    #[prop(into, optional)] description: Signal<String>,
    #[prop(into, optional)] name: Signal<String>,
    #[prop(into, optional)] placeholder: Signal<String>,
    #[prop(into, optional)] value: RwSignal<String>,
    #[prop(optional)] error: bool,
    #[prop(into, optional)] disabled: Signal<bool>,
) -> impl IntoView {
    view! {
        <div class="field">
            <Show when=move || !label.get().is_empty()>
                <label class="field-label" for=move || name.get()>{move || label.get()}</label>
            </Show>
            <input
                type="text"
                id=move || name.get()
                name=move || name.get()
                disabled=move || disabled.get()
                aria-invalid=error.to_string()
                aria-describedby=move || if description.get().is_empty() { None } else { Some(format!("{}-hint", name.get())) }
                class=format!("field-input {}", if error { "has-error" } else { "" })
                placeholder=move || placeholder.get()
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
            <Show when=move || !description.get().is_empty()>
                <p id=move || format!("{}-hint", name.get()) class=format!("field-hint {}", if error { "is-error" } else { "" })>
                    {move || description.get()}
                </p>
            </Show>
        </div>
    }
}

/// Multi-line variant for pasting long payloads (Cashu tokens).
#[component]
pub fn Textarea(
    #[prop(into, optional)] label: Signal<String>,
    #[prop(into, optional)] description: Signal<String>,
    #[prop(into, optional)] name: Signal<String>,
    #[prop(into, optional)] placeholder: Signal<String>,
    #[prop(into, optional)] value: RwSignal<String>,
    #[prop(optional)] error: bool,
    #[prop(into, optional)] disabled: Signal<bool>,
) -> impl IntoView {
    view! {
        <div class="field">
            <Show when=move || !label.get().is_empty()>
                <label class="field-label" for=move || name.get()>{move || label.get()}</label>
            </Show>
            <textarea
                spellcheck="false"
                autocomplete="off"
                autocapitalize="off"
                id=move || name.get()
                name=move || name.get()
                disabled=move || disabled.get()
                aria-invalid=error.to_string()
                aria-describedby=move || if description.get().is_empty() { None } else { Some(format!("{}-hint", name.get())) }
                class=format!("field-textarea {}", if error { "has-error" } else { "" })
                placeholder=move || placeholder.get()
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            ></textarea>
            <Show when=move || !description.get().is_empty()>
                <p id=move || format!("{}-hint", name.get()) class=format!("field-hint {}", if error { "is-error" } else { "" })>
                    {move || description.get()}
                </p>
            </Show>
        </div>
    }
}
