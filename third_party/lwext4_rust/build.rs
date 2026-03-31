use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("failed to create dir");
    for entry in fs::read_dir(src).expect("failed to read dir") {
        let entry = entry.expect("failed to read entry");
        let ty = entry.file_type().expect("failed to get file type");
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).expect("failed to copy file");
        }
    }
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Copy the C source tree into OUT_DIR so build.rs doesn't modify the source directory
    let c_src = manifest_dir.join("c");
    let c_out = out_dir.join("c");
    if c_out.exists() {
        fs::remove_dir_all(&c_out).ok();
    }
    copy_dir_all(&c_src, &c_out);

    let lwext4_out = c_out.join("lwext4");
    let lwext4_out = lwext4_out
        .canonicalize()
        .expect("cannot canonicalize lwext4 output path");

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let target_abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    let lwext4_lib = &format!("lwext4-{arch}");

    // Build the C library in OUT_DIR
    {
        let status = Command::new("make")
            .args([
                "musl-generic",
                "-C",
                lwext4_out.to_str().expect("invalid path of lwext4"),
            ])
            .arg(format!("ARCH={arch}"))
            .arg(format!("TARGET_ABI={target_abi}"))
            .arg(format!(
                "ULIBC={}",
                if cfg!(feature = "std") { "OFF" } else { "ON" }
            ))
            .status()
            .expect("failed to execute process: make lwext4");
        assert!(status.success());
    }

    // Generate Rust bindings
    {
        let cc = &format!("{arch}-linux-musl-gcc");
        let output = Command::new(cc)
            .args(["-print-sysroot"])
            .output()
            .expect("failed to execute process: gcc -print-sysroot");

        let sysroot = core::str::from_utf8(&output.stdout).unwrap();
        let sysroot = sysroot.trim_end();
        let sysroot_inc = &format!("-I{sysroot}/include/");

        generates_bindings_to_rust(sysroot_inc, &c_out, &lwext4_out);
    }

    println!("cargo:rustc-link-lib=static={lwext4_lib}");
    println!(
        "cargo:rustc-link-search=native={}",
        lwext4_out.to_str().unwrap()
    );
    println!("cargo:rerun-if-changed=c/wrapper.h");
    println!("cargo:rerun-if-changed=c/lwext4/src");
}

fn generates_bindings_to_rust(mpath: &str, c_out: &Path, lwext4_out: &Path) {
    let target = env::var("TARGET").unwrap();
    if target.ends_with("-softfloat") {
        // Clang does not recognize the `-softfloat` suffix
        unsafe { env::set_var("TARGET", target.replace("-softfloat", "")) };
    }

    let wrapper_h = c_out.join("wrapper.h");
    let include_dir = lwext4_out.join("include");
    let build_include_dir = lwext4_out.join("build_musl-generic/include");

    let bindings = bindgen::Builder::default()
        .use_core()
        .wrap_unsafe_ops(true)
        .header(wrapper_h.to_str().unwrap())
        .clang_arg(mpath)
        .clang_arg(format!("-I{}", include_dir.to_str().unwrap()))
        .clang_arg(format!("-I{}", build_include_dir.to_str().unwrap()))
        .layout_tests(false)
        .parse_callbacks(Box::new(CustomCargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");

    // Restore the original target environment variable
    unsafe { env::set_var("TARGET", target) };

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

#[derive(Debug)]
struct CustomCargoCallbacks;
impl bindgen::callbacks::ParseCallbacks for CustomCargoCallbacks {
    fn header_file(&self, filename: &str) {
        add_include(filename);
    }

    fn include_file(&self, filename: &str) {
        add_include(filename);
    }

    fn read_env_var(&self, key: &str) {
        println!("cargo:rerun-if-env-changed={key}");
    }
}

fn add_include(filename: &str) {
    if !Path::new(filename).ends_with("ext4_config.h") {
        println!("cargo:rerun-if-changed={filename}");
    }
}
