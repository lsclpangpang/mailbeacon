fn main() {
    // Build scripts run on the host, so inspect Cargo's target configuration.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        // MSVC reserves only 1 MiB by default. The GUI IPC dispatch can exhaust
        // that stack before an async command reaches its first network call.
        // Reserve 8 MiB for this EXE, retaining the default small initial commit;
        // do not change dependency DLLs or Tokio worker stack configuration.
        println!("cargo:rustc-link-arg-bin=mailbeacon-app=/STACK:8388608");
    }
    tauri_build::build()
}
