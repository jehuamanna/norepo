use dioxus::prelude::*;

use crate::rbag::api::{ApiClient, ChangePasswordRequest};
use crate::rbag::state::AppState;
use crate::rbag::types::LoginResponse;

#[component]
pub fn ChangePasswordScreen(reset_token: String) -> Element {
    let mut new_password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let mut state = use_context::<Signal<AppState>>();
    let client = use_context::<Signal<ApiClient>>();
    let token = reset_token.clone();

    let on_submit = move |e: FormEvent| {
        e.prevent_default();
        if new_password() != confirm() {
            error.set(Some("passwords do not match".into()));
            return;
        }
        if new_password().len() < 8 {
            error.set(Some("password must be ≥ 8 characters".into()));
            return;
        }
        let token = token.clone();
        let new_pw = new_password();
        let api = client();
        spawn(async move {
            match api
                .change_password(ChangePasswordRequest {
                    reset_token: token,
                    new_password: new_pw,
                })
                .await
            {
                Ok(LoginResponse::Ok {
                    session_token,
                    user_id,
                    ..
                }) => {
                    state.with_mut(|s| {
                        s.session_token = Some(session_token);
                        s.identity = Some(crate::rbag::types::MePayload {
                            user_id,
                            ..Default::default()
                        });
                    });
                }
                Ok(LoginResponse::MustChangePassword { .. }) => {
                    error.set(Some("server still requires change".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    rsx! {
        form { onsubmit: on_submit,
            label { "New password"
                input {
                    r#type: "password",
                    value: "{new_password}",
                    oninput: move |evt| new_password.set(evt.value()),
                    required: true,
                }
            }
            label { "Confirm"
                input {
                    r#type: "password",
                    value: "{confirm}",
                    oninput: move |evt| confirm.set(evt.value()),
                    required: true,
                }
            }
            if let Some(msg) = error.read().clone() {
                p { class: "error", "{msg}" }
            }
            button { r#type: "submit", "Set password" }
        }
    }
}
