//! Link the bridge library built inside a pinned Cycles tree, when one has been provisioned.
//! Without it the crate still builds, and `Session::new` reports how to provision. See README.md.
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(cycles_sdk)");
    println!("cargo:rerun-if-env-changed=N4_CYCLES_DIR");
    println!("cargo:rerun-if-env-changed=OPENUSD_DIR");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let explicit = std::env::var_os("N4_CYCLES_DIR").map(PathBuf::from);
    let dir = explicit
        .clone()
        .unwrap_or_else(|| manifest.join("../../../.local/cycles/install"));
    let lib = dir.join("libn4cycles.dylib");
    if !lib.is_file() {
        // Cargo treats a missing path as always changed, which would rebuild this crate and its
        // dependents on every invocation. provision.sh touches this script instead.
        println!("cargo:rerun-if-changed=build.rs");
        assert!(
            explicit.is_none() && std::env::var_os("CARGO_FEATURE_REQUIRE_SDK").is_none(),
            "Cycles bundle required: {} is missing (run tools/provision.sh or set N4_CYCLES_DIR)",
            lib.display()
        );
        return;
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", lib.display());
    let dir = dir.canonicalize().expect("resolve Cycles bundle");
    println!("cargo:rustc-cfg=cycles_sdk");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=dylib=n4cycles");
    println!("cargo::metadata=libdir={}", dir.display());
    // The bundle is found at run time by an rpath on the final executable. Dependents add it from
    // DEP_N4CYCLES_LIBDIR; this crate's own tests and examples get it here, with the USD reader's.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
    let usd = std::env::var_os("OPENUSD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../engine/target/openusd-sdk"))
        .join("lib");
    if usd.is_dir() {
        println!("cargo:rustc-link-arg-examples=-Wl,-rpath,{}", usd.display());
    }
}
