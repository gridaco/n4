use std::path::PathBuf;

fn build_webp_animation() {
    println!("cargo:rerun-if-changed=native/webp_animation.c");
    println!("cargo:rerun-if-env-changed=WEBP_DIR");
    let prefix = std::env::var_os("WEBP_DIR").map(PathBuf::from).or_else(|| {
        ["/opt/homebrew/opt/webp", "/usr/local/opt/webp"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.join("include/webp/demux.h").is_file())
    });
    let mut compiler = cc::Build::new();
    if let Some(prefix) = prefix {
        compiler.include(prefix.join("include"));
        println!(
            "cargo:rustc-link-search=native={}",
            prefix.join("lib").display()
        );
    }
    compiler.file("native/webp_animation.c").compile("n4_webp");
    println!("cargo:rustc-link-lib=webpdemux");
}

fn main() {
    build_webp_animation();
    // Cargo links the adapter library transitively, but an rpath belongs to the final executable.
    println!("cargo:rustc-check-cfg=cfg(n4_openusd)");
    if let Some(libdir) = std::env::var_os("DEP_NOTHING_OPENUSD_LIBDIR") {
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath,{}",
            PathBuf::from(libdir).display()
        );
        println!("cargo:rustc-cfg=n4_openusd");
    }
    if let Some(libdir) = std::env::var_os("DEP_N4CYCLES_LIBDIR") {
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath,{}",
            PathBuf::from(libdir).display()
        );
    }
    println!("cargo:rerun-if-changed=native/video_decoder.c");
    if std::env::var_os("CARGO_FEATURE_NATIVE_DECODE").is_none() {
        return;
    }
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let prefix = std::env::var_os("FFMPEG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            ["/opt/homebrew/opt/ffmpeg", "/usr/local/opt/ffmpeg"]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.join("include/libavcodec/avcodec.h").is_file())
        });
    let mut compiler = cc::Build::new();
    if let Some(prefix) = prefix {
        compiler.include(prefix.join("include"));
        println!(
            "cargo:rustc-link-search=native={}",
            prefix.join("lib").display()
        );
    }
    compiler.file("native/video_decoder.c").compile("n4_video");
    for library in ["avfilter", "avcodec", "avformat", "avutil"] {
        println!("cargo:rustc-link-lib={library}");
    }
}
