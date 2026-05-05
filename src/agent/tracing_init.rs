#[cfg(not(target_arch = "wasm32"))]
pub fn init(filter: Option<&str>) {
    use tracing_subscriber::EnvFilter;
    let default = filter.unwrap_or("info,operon_dioxus=debug");
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .try_init();
}

#[cfg(target_arch = "wasm32")]
pub fn init(_filter: Option<&str>) {
    tracing_wasm::set_as_global_default();
}
