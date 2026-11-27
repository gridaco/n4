use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(openusd_sdk)");
    println!("cargo:rerun-if-changed=native/usd_mesh.cpp");
    println!("cargo:rerun-if-env-changed=OPENUSD_DIR");
    if std::env::var_os("CARGO_FEATURE_SDK").is_none() {
        return;
    }
    let local = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../engine/target/openusd-sdk");
    let explicit = std::env::var_os("OPENUSD_DIR");
    let prefix = explicit.as_ref().map(PathBuf::from).unwrap_or(local);
    let header = prefix.join("include/pxr/pxr.h");
    // Track provisioning even if a previous build had no SDK.
    println!("cargo:rerun-if-changed={}", header.display());
    if !header.is_file() {
        assert!(
            explicit.is_none() && std::env::var_os("CARGO_FEATURE_REQUIRE_SDK").is_none(),
            "OpenUSD SDK required: header missing at {} (set OPENUSD_DIR)",
            header.display()
        );
        return;
    }
    let prefix = prefix.canonicalize().expect("resolve OpenUSD SDK");
    let include = prefix.join("include");
    let mut compiler = cc::Build::new();
    compiler
        .cpp(true)
        .std("c++17")
        .flag("-isystem")
        .flag(&include);
    let mut python = None;
    for item in std::fs::read_dir(&include)
        .expect("read OpenUSD includes")
        .flatten()
    {
        if item.path().join("Python.h").is_file() {
            compiler.include(item.path());
            python = Some(item.file_name());
        }
    }
    compiler
        .file("native/usd_mesh.cpp")
        .compile("nothing_usd_bridge");
    let lib = prefix.join("lib");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo::metadata=libdir={}", lib.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    let monolithic = ["libusd_ms.dylib", "libusd_ms.so"]
        .iter()
        .any(|f| lib.join(f).is_file());
    if monolithic {
        println!("cargo:rustc-link-lib=usd_ms");
    } else {
        for name in [
            "usdShade", "usdGeom", "usd", "sdf", "ar", "vt", "gf", "tf", "arch",
        ] {
            println!("cargo:rustc-link-lib=usd_{name}");
        }
    }
    if let Some(name) = python {
        // A Python-enabled OpenUSD SDK's headers inline a few Python C-API symbols.
        println!("cargo:rustc-link-lib={}", name.to_string_lossy());
        if !monolithic
            && ["libusd_python.dylib", "libusd_python.so"]
                .iter()
                .any(|f| lib.join(f).is_file())
        {
            println!("cargo:rustc-link-lib=usd_python");
        }
    }
    println!("cargo:rustc-cfg=openusd_sdk");
}
