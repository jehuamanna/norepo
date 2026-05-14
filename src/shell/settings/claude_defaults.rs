//! App-wide Claude defaults section in Settings.
//!
//! The bottom tier of the three-tier hierarchy
//! (chat → project → global → omit-flag). Writes to
//! `local_app_settings` rows `claude.default_model` and
//! `claude.default_permission_mode`. Per-chat and per-project pickers
//! inherit from here unless they override.
//!
//! Bumping `GLOBAL_SETTINGS_VERSION` after a write triggers any open
//! chat's `picker_persisted` memo to recompute its "Inherit (X)" label.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::local_mode::desktop::LocalSettingsRepo;

#[component]
pub fn ClaudeDefaultsSection() -> Element {
    let LocalSettingsRepo(settings_repo) = use_context();

    // Bumped per-write so the dropdowns re-read the persisted value
    // after a change without going through a full re-mount.
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);

    let current_model = {
        let _ = refresh_token.read();
        settings_repo
            .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_MODEL)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    };
    let current_perm = {
        let _ = refresh_token.read();
        settings_repo
            .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_PERMISSION_MODE)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    };

    let settings_repo_for_model = settings_repo.clone();
    let settings_repo_for_perm = settings_repo.clone();

    rsx! {
        div {
            style: "margin-top: 1rem;",
            "data-testid": "settings-claude-defaults",
            h3 {
                style: "margin: 0 0 0.5rem 0; font-size: 0.95em;",
                "Claude defaults"
            }
            p {
                class: "operon-modal-help",
                style: "font-size: 0.8em; color: var(--operon-fg-muted, #666); margin: 0 0 0.5rem 0;",
                "Defaults for every project and chat. Per-project overrides live in Tools → Project Claude Defaults; per-chat overrides live in the chat header."
            }
            div {
                style: "display: flex; gap: 10px; align-items: center; margin-bottom: 6px;",
                label { style: "min-width: 140px; font-size: 0.9em;", "Model:" }
                select {
                    class: "operon-companion-model-picker",
                    "data-testid": "settings-claude-model-picker",
                    onchange: move |e| {
                        let v = e.value();
                        let next = if v == "inherit" { "" } else { v.as_str() };
                        if let Err(e) = settings_repo_for_model.set(
                            crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_MODEL,
                            next,
                        ) {
                            tracing::warn!(
                                target: "operon::settings",
                                "persist global claude model failed: {e}"
                            );
                        }
                        refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                        *crate::shell::companion_state::GLOBAL_SETTINGS_VERSION.write() += 1;
                    },
                    option { value: "inherit",
                        selected: current_model.is_none(),
                        "Inherit (Claude default)"
                    }
                    option { value: "claude-opus-4-7",
                        selected: current_model.as_deref() == Some("claude-opus-4-7"),
                        "Opus 4.7"
                    }
                    option { value: "claude-opus-4-6",
                        selected: current_model.as_deref() == Some("claude-opus-4-6"),
                        "Opus 4.6"
                    }
                    option { value: "claude-sonnet-4-6",
                        selected: current_model.as_deref() == Some("claude-sonnet-4-6"),
                        "Sonnet 4.6"
                    }
                    option { value: "claude-sonnet-4-5",
                        selected: current_model.as_deref() == Some("claude-sonnet-4-5"),
                        "Sonnet 4.5"
                    }
                    option { value: "claude-haiku-4-5",
                        selected: current_model.as_deref() == Some("claude-haiku-4-5"),
                        "Haiku 4.5"
                    }
                    option { value: "claude-3-5-sonnet-20241022",
                        selected: current_model.as_deref() == Some("claude-3-5-sonnet-20241022"),
                        "Sonnet 3.5"
                    }
                    option { value: "claude-3-5-haiku-20241022",
                        selected: current_model.as_deref() == Some("claude-3-5-haiku-20241022"),
                        "Haiku 3.5"
                    }
                    option { value: "claude-3-opus-20240229",
                        selected: current_model.as_deref() == Some("claude-3-opus-20240229"),
                        "Opus 3"
                    }
                }
            }
            div {
                style: "display: flex; gap: 10px; align-items: center;",
                label { style: "min-width: 140px; font-size: 0.9em;", "Permission mode:" }
                select {
                    class: "operon-companion-model-picker",
                    "data-testid": "settings-claude-permission-picker",
                    onchange: move |e| {
                        let v = e.value();
                        let next = if v == "inherit" { "" } else { v.as_str() };
                        if let Err(e) = settings_repo_for_perm.set(
                            crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_PERMISSION_MODE,
                            next,
                        ) {
                            tracing::warn!(
                                target: "operon::settings",
                                "persist global claude permission_mode failed: {e}"
                            );
                        }
                        refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                        *crate::shell::companion_state::GLOBAL_SETTINGS_VERSION.write() += 1;
                    },
                    option { value: "inherit",
                        selected: current_perm.is_none(),
                        "Inherit (Claude default)"
                    }
                    option { value: "acceptEdits",
                        selected: current_perm.as_deref() == Some("acceptEdits"),
                        "Accept edits"
                    }
                    option { value: "plan",
                        selected: current_perm.as_deref() == Some("plan"),
                        "Plan"
                    }
                    option { value: "bypassPermissions",
                        selected: current_perm.as_deref() == Some("bypassPermissions"),
                        "Bypass"
                    }
                }
            }
        }
    }
}
