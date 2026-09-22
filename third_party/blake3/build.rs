fn main() {
    // Rust intrinsics only. BLAKE3 output matches the C/assembly backends.
    let cfgs = [
        "blake3_sse2_ffi",
        "blake3_sse2_rust",
        "blake3_sse41_ffi",
        "blake3_sse41_rust",
        "blake3_avx2_ffi",
        "blake3_avx2_rust",
        "blake3_avx512_ffi",
        "blake3_neon",
        "blake3_wasm32_simd",
    ];
    for cfg_name in cfgs {
        println!("cargo::rustc-check-cfg=cfg({cfg_name}, values(none()))");
    }

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if arch == "x86_64" || arch == "x86" {
        println!("cargo::rustc-cfg=blake3_sse2_rust");
        println!("cargo::rustc-cfg=blake3_sse41_rust");
        println!("cargo::rustc-cfg=blake3_avx2_rust");
    }
    if arch == "wasm32" && std::env::var_os("CARGO_FEATURE_WASM32_SIMD").is_some() {
        println!("cargo::rustc-cfg=blake3_wasm32_simd");
    }
}
