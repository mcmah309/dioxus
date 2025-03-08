use std::env;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let is_wasm = target.contains("wasm32");
    let env_filter_enabled = env::var("CARGO_FEATURE_ENV_FILTER").is_ok();

    if is_wasm && env_filter_enabled {
        if let Ok(rust_log) = env::var("RUST_LOG") {
            println!("cargo:rustc-env=RUST_LOG_BUILD_TIME={}", rust_log);
        }
    }
}
