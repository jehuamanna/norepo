use dioxus::prelude::*;

use crate::rbag::api::{ApiClient, LoginRequest};
use crate::rbag::state::AppState;
use crate::rbag::types::LoginResponse;

#[component]
pub fn LoginScreen() -> Element {
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut must_change: Signal<Option<String>> = use_signal(|| None);

    let mut state = use_context::<Signal<AppState>>();
    let client = use_context::<Signal<ApiClient>>();

    let on_submit = move |e: FormEvent| {
        e.prevent_default();
        let email_val = email();
        let pass_val = password();
        let api = client();
        spawn(async move {
            match api
                .login(LoginRequest {
                    email: email_val,
                    password: pass_val,
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
                        // Identity will be hydrated by the next /me call.
                        s.identity = Some(crate::rbag::types::MePayload {
                            user_id,
                            ..Default::default()
                        });
                    });
                }
                Ok(LoginResponse::MustChangePassword { reset_token }) => {
                    must_change.set(Some(reset_token));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    rsx! {
        div { class: "rbag-auth-card",
            h1 { "Sign in" }
            if let Some(token) = must_change.read().clone() {
                p { class: "warn", "You must change your password before continuing." }
                crate::rbag::screens::change_password::ChangePasswordScreen { reset_token: token }
            } else {
                form { onsubmit: on_submit,
                    label { "Email"
                        input {
                            r#type: "email",
                            value: "{email}",
                            oninput: move |evt| email.set(evt.value()),
                            required: true,
                        }
                    }
                    label { "Password"
                        input {
                            r#type: "password",
                            value: "{password}",
                            oninput: move |evt| password.set(evt.value()),
                            required: true,
                        }
                    }
                    if let Some(msg) = error.read().clone() {
                        p { class: "error", "{msg}" }
                    }
                    button { r#type: "submit", "Sign in" }
                }
                p {
                    a { href: "/forgot-password", "Forgot your password?" }
                }
            }
        }
    }
}
