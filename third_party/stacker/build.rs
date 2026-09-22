use std::env;

fn main() {
    let target = env::var("TARGET").unwrap();
    let mut cfg = cbuild::Build::new();
    if target.contains("windows") {
        cfg.define("WINDOWS", None);
        cfg.file("src/arch/windows.c");
        cfg.include("src/arch").compile("libstacker.a");
    }
}
