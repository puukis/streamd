use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(have_nvenc)");
    println!("cargo:rerun-if-env-changed=NVENC_HEADER_PATH");
    println!("cargo:rerun-if-env-changed=NVENC_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=NVENC_LIB_DIR");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if let Some(header) = locate_nvenc_header(&target_os) {
        let include_dir = env::var_os("NVENC_INCLUDE_DIR")
            .map(PathBuf::from)
            .or_else(|| header.parent().map(Path::to_path_buf));
        generate_nvenc_bindings(&header, include_dir.as_deref());
        emit_nvenc_link_instructions(&target_os);
    } else {
        println!("cargo:warning=NVENC headers were not found for target {target_os}");
        println!(
            "cargo:warning=Set NVENC_HEADER_PATH or install nvEncodeAPI.h in a standard location."
        );
    }
}

fn locate_nvenc_header(target_os: &str) -> Option<PathBuf> {
    if let Some(path) = env::var_os("NVENC_HEADER_PATH").map(PathBuf::from) {
        return path.exists().then_some(path);
    }

    match target_os {
        "linux" => {
            let path = PathBuf::from("/usr/local/include/ffnvcodec/nvEncodeAPI.h");
            path.exists().then_some(path)
        }
        "windows" => env::var_os("CUDA_PATH")
            .map(PathBuf::from)
            .map(|cuda| cuda.join("include").join("nvEncodeAPI.h"))
            .filter(|path| path.exists()),
        _ => None,
    }
}

fn emit_nvenc_link_instructions(target_os: &str) {
    if let Some(dir) = env::var_os("NVENC_LIB_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            PathBuf::from(dir).display()
        );
    }

    match target_os {
        "linux" => {
            println!("cargo:rustc-link-search=native=/usr/lib");
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            println!("cargo:rustc-link-lib=dylib=nvidia-encode");
        }
        "windows" => {
            // The Windows runtime path uses dynamic loading for NVENC.
        }
        _ => {}
    }
}

fn generate_nvenc_bindings(header: &Path, include_dir: Option<&Path>) {
    println!("cargo:rerun-if-changed={}", header.display());

    let mut builder = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .allowlist_type("NV_ENC_.*")
        .allowlist_type("NVENCSTATUS")
        .allowlist_type("NV_ENCODE_API_FUNCTION_LIST")
        .allowlist_function("NvEncodeAPICreateInstance")
        .allowlist_var("NV_ENC_.*")
        .prepend_enum_name(false)
        .derive_debug(true)
        .derive_default(true);

    if let Some(include_dir) = include_dir {
        builder = builder.clang_arg(format!("-I{}", include_dir.display()));
    }

    let bindings = builder
        .generate()
        .expect("Unable to generate NVENC bindings");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("nvenc_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("Couldn't write NVENC bindings");

    println!("cargo:rustc-cfg=have_nvenc");
}
