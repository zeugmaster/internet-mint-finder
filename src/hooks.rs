//! Small reactive helpers shared by pages and components.

use leptos::prelude::*;
use std::time::Duration;

/// Returns a signal that increments every `secs` seconds in the browser.
///
/// Use it as a `Resource` source to poll server data. During SSR the signal
/// stays at 0 (effects only run on the client), so server-rendered HTML is
/// stable and hydration kicks off live updates.
pub fn use_poll(secs: u64) -> ReadSignal<u32> {
    let (tick, set_tick) = signal(0u32);

    Effect::new(move |_| {
        if let Ok(handle) = set_interval_with_handle(
            move || {
                #[cfg(feature = "hydrate")]
                if document().hidden() {
                    return;
                }
                let _ = set_tick.try_update(|t| *t = t.wrapping_add(1));
            },
            Duration::from_secs(secs.max(1)),
        ) {
            on_cleanup(move || handle.clear());
        }
    });

    tick
}
