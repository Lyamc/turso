use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/kvstore.c");
    println!("cargo:rerun-if-changed=include");
    compile_kvstore();
}

fn compile_kvstore() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let src = Path::new("src/kvstore.c");
    let is_msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");

    if is_msvc {
        let obj = out_dir.join("kvstore.obj");
        let lib = out_dir.join("kvstore.lib");
        run(
            Command::new(std::env::var("CC").unwrap_or_else(|_| "cl".into()))
                .args(["/nologo", "/c", "/Fo"])
                .arg(&obj)
                .arg(src),
        );
        run(
            Command::new(std::env::var("AR").unwrap_or_else(|_| "lib".into()))
                .args(["/nologo", "/OUT:"])
                .arg(&lib)
                .arg(&obj),
        );
    } else {
        let obj = out_dir.join("kvstore.o");
        let lib = out_dir.join("libkvstore.a");
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        run(Command::new(&cc)
            .args(["-c", "-o"])
            .arg(&obj)
            .arg("-Wno-unused-parameter")
            .arg("-I")
            .arg("include")
            .arg(src));
        let ar = std::env::var("AR").unwrap_or_else(|_| "ar".into());
        run(Command::new(&ar).args(["rcs"]).arg(&lib).arg(&obj));
    }

    println!("cargo:rustc-link-lib=static=kvstore");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
}

fn run(command: &mut Command) {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command.status().unwrap_or_else(|err| {
        panic!("failed to run {program}: {err}");
    });
    if !status.success() {
        panic!("{program} failed with {status}");
    }
}
