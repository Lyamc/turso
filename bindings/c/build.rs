use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/varargs.c");
    compile_varargs();
    let profile_dir = target_profile_dir();
    println!("cargo:rustc-link-search=native={}", profile_dir.display());

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        configure_msvc_sqlite3();
    }
}

fn compile_varargs() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let src = Path::new("src/varargs.c");
    let is_msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");

    if is_msvc {
        let obj = out_dir.join("varargs.obj");
        let lib = out_dir.join("turso_sqlite3_varargs.lib");
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
        println!("cargo:rustc-link-lib=static=turso_sqlite3_varargs");
        println!("cargo:rustc-link-search=native={}", out_dir.display());
    } else {
        let obj = out_dir.join("varargs.o");
        let lib = out_dir.join("libturso_sqlite3_varargs.a");
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        run(Command::new(&cc).args(["-c", "-o"]).arg(&obj).arg(src));
        let ar = std::env::var("AR").unwrap_or_else(|_| "ar".into());
        run(Command::new(&ar).args(["rcs"]).arg(&lib).arg(&obj));
        println!("cargo:rustc-link-lib=static=turso_sqlite3_varargs");
        println!("cargo:rustc-link-search=native={}", out_dir.display());
    }
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

fn target_profile_dir() -> PathBuf {
    let out_dir = PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR for build scripts"),
    );
    out_dir
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("build"))
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .expect("OUT_DIR must be inside Cargo's target profile build directory")
}

fn configure_msvc_sqlite3() {
    for variable in [
        "VCPKG_ROOT",
        "VCPKGRS_TRIPLET",
        "VCPKGRS_DYNAMIC",
        "VCPKGRS_DISABLE",
        "VCPKGRS_NO_SQLITE3",
        "SQLITE3_NO_VCPKG",
        "NO_VCPKG",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    if std::env::var_os("CARGO_FEATURE_SQLITE3").is_none() {
        return;
    }

    let library = vcpkg::find_package("sqlite3").unwrap_or_else(|error| {
        panic!(
            "failed to find static SQLite with vcpkg: {error}; \
             the `sqlite3` feature on MSVC requires \
             `vcpkg install sqlite3:x64-windows-static-md`"
        )
    });
    assert!(
        library.is_static,
        "MSVC SQLite compatibility tests require static vcpkg linkage"
    );
}
