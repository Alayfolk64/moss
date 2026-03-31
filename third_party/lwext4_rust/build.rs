use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let c_path = PathBuf::from("c/lwext4")
        .canonicalize()
        .expect("cannot canonicalize path");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let lwext4_lib = &format!("lwext4-{arch}");
    {
        let status = Command::new("make")
            .args([
                "musl-generic",
                "-C",
                c_path.to_str().expect("invalid path of lwext4"),
            ])
            .arg(format!("ARCH={arch}"))
            .arg(format!(
                "ULIBC={}",
                if env::var("CARGO_FEATURE_STD").is_ok() {
                    "OFF"
                } else {
                    "ON"
                }
            ))
            .arg(format!("OUT_DIR={}", out_dir.display()))
            .status()
            .expect("failed to execute process: make lwext4");
        assert!(status.success());
    }

    println!("cargo:rustc-link-lib=static={lwext4_lib}");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rerun-if-changed=c/wrapper.h");
    println!("cargo:rerun-if-changed={}/src", c_path.to_str().unwrap());
    println!("cargo:rerun-if-changed=src/bindings.rs");
}
