use pimalaya_tui::build::{features_env, git_envs, target_envs};

fn main() {
    features_env(include_str!("./Cargo.toml"));
    target_envs();
    git_envs();

    // Optional compile-time overrides. Each is read by `option_env!` in the
    // crate, so re-export it from the build script's environment and trigger a
    // rebuild whenever the value changes.
    for var in [
        "HIMALAYA_FROM_EMAIL",
        "HIMALAYA_FROM_NAME",
        "HIMALAYA_CC_EMAIL",
        "HIMALAYA_DOMAIN",
        "HIMALAYA_EXCLUDED_RECIPIENTS",
    ] {
        println!("cargo::rerun-if-env-changed={var}");
        if let Ok(val) = std::env::var(var) {
            println!("cargo::rustc-env={var}={val}");
        }
    }
}
