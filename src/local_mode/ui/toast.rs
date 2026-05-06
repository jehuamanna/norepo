//! Plans-Phase-8-explorer-undo: minimal toast / snackbar primitive.
//!
//! Fed by an app-scope `Signal<Option<Toast>>`. The host component
//! [`ToastHost`] renders the message in a corner-anchored card with a
//! 3-second auto-dismiss timer. There is exactly one slot — a fresh
//! `set_toast(Some(...))` replaces any pending toast (good enough for the
//! single-message use case; extend to a queue later if multiple events
//! pile up).

use dioxus::prelude::*;

#[derive(Clone, PartialEq, Debug)]
pub struct Toast {
    pub message: String,
    /// "info" / "warn" / "error" — affects the accent colour.
    pub kind: ToastKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Info,
    Warn,
    Error,
}

/// App-scope context handle. The explorer's failed-undo path writes here;
/// `ToastHost` reads + auto-clears.
#[derive(Clone, Copy)]
pub struct ToastSlot(pub Signal<Option<Toast>>);

#[component]
pub fn ToastHost() -> Element {
    let ToastSlot(mut slot) = use_context();
    let current = slot.read().clone();

    // Auto-dismiss: spawn a 3 s timer whenever a fresh toast lands.
    use_effect(move || {
        let now = slot.read().clone();
        if now.is_some() {
            spawn(async move {
                futures_timer::Delay::new(std::time::Duration::from_millis(3000)).await;
                // Only clear if the same toast is still in the slot —
                // otherwise a newer toast that arrived in the interim
                // would be wiped.
                let still_same = slot.read().clone() == now;
                if still_same {
                    slot.set(None);
                }
            });
        }
    });

    let Some(t) = current else {
        return rsx! { span {} };
    };
    let kind_class = match t.kind {
        ToastKind::Info => "operon-toast operon-toast-info",
        ToastKind::Warn => "operon-toast operon-toast-warn",
        ToastKind::Error => "operon-toast operon-toast-error",
    };
    let testid = match t.kind {
        ToastKind::Info => "toast-info",
        ToastKind::Warn => "toast-warn",
        ToastKind::Error => "toast-error",
    };
    rsx! {
        div {
            class: "{kind_class}",
            "data-testid": "{testid}",
            role: "status",
            "aria-live": "polite",
            "{t.message}"
        }
    }
}
