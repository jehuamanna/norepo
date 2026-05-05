fn main() {
    operon_dioxus::agent::tracing_init::init(None);
    dioxus::launch(operon_dioxus::app::App);
}
