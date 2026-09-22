fn main() {
    // The patched crate always uses the pure-Rust AEGIS implementation.
    println!("cargo:rerun-if-changed=build.rs");
}
