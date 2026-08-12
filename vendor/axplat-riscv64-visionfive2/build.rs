fn main() {
    println!("cargo:rerun-if-env-changed=AX_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=MOSS_BUILD_EPOCH");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    if let Ok(config_path) = std::env::var("AX_CONFIG_PATH") {
        println!("cargo:rerun-if-changed={config_path}");
    }

    let build_epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            std::env::var("MOSS_BUILD_EPOCH")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("host clock predates the Unix epoch")
                .as_secs()
        });
    println!("cargo:rustc-env=MOSS_BUILD_EPOCH={build_epoch}");
}
