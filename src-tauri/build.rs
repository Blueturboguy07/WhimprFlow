fn main() {
    // The publik app token is read at compile time (`option_env!("PUBLIK_APP_TOKEN")`
    // in src/publik.rs). Cargo's fingerprint does not include env vars unless told,
    // so without this line a cached object keeps whatever value it had — the
    // classic "the release shipped with no token" failure.
    println!("cargo:rerun-if-env-changed=PUBLIK_APP_TOKEN");
    tauri_build::build();
}
