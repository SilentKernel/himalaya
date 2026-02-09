use pimalaya_tui::build::{features_env, git_envs, target_envs};

fn main() {
    features_env(include_str!("./Cargo.toml"));
    target_envs();
    git_envs();

    println!("cargo::rerun-if-env-changed=HIMALAYA_FROM_EMAIL");
    println!("cargo::rerun-if-env-changed=HIMALAYA_FROM_NAME");
    if let Ok(val) = std::env::var("HIMALAYA_FROM_EMAIL") {
        println!("cargo::rustc-env=HIMALAYA_FROM_EMAIL={val}");
    }
    if let Ok(val) = std::env::var("HIMALAYA_FROM_NAME") {
        println!("cargo::rustc-env=HIMALAYA_FROM_NAME={val}");
    }
}
