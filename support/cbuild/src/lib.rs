//! Compile C and assembly the way the `cc` crate does, without depending on it.
//!
//! The subset of `cc::Build` used by patched third-party build scripts is
//! implemented here. Link lines and library names follow `cc`'s rules so the
//! existing build scripts keep working.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug)]
pub struct Tool {
    path: PathBuf,
    msvc: bool,
}

impl Tool {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_like_msvc(&self) -> bool {
        self.msvc
    }

    pub fn is_like_gnu(&self) -> bool {
        !self.msvc
    }

    pub fn is_like_clang(&self) -> bool {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().contains("clang"))
    }
}

#[derive(Clone, Debug)]
pub struct Build {
    files: Vec<PathBuf>,
    includes: Vec<PathBuf>,
    definitions: Vec<(String, Option<String>)>,
    flags: Vec<OsString>,
    cpp: bool,
    std: Option<String>,
    warnings: bool,
    opt_level: Option<String>,
    static_crt: bool,
    cargo_metadata: bool,
    out_dir: Option<PathBuf>,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            includes: Vec::new(),
            definitions: Vec::new(),
            flags: Vec::new(),
            cpp: false,
            std: None,
            warnings: true,
            opt_level: None,
            static_crt: false,
            cargo_metadata: true,
            out_dir: None,
        }
    }
}

impl Build {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn file<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.files.push(path.as_ref().into());
        self
    }

    pub fn files<P>(&mut self, paths: P) -> &mut Self
    where
        P: IntoIterator,
        P::Item: AsRef<Path>,
    {
        for path in paths {
            self.file(path);
        }
        self
    }

    pub fn include<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.includes.push(path.as_ref().into());
        self
    }

    pub fn define<'a, V: Into<Option<&'a str>>>(&mut self, var: &str, val: V) -> &mut Self {
        self.definitions
            .push((var.to_string(), val.into().map(str::to_string)));
        self
    }

    pub fn flag(&mut self, flag: impl AsRef<OsStr>) -> &mut Self {
        self.flags.push(flag.as_ref().to_os_string());
        self
    }

    pub fn flag_if_supported(&mut self, flag: impl AsRef<OsStr>) -> &mut Self {
        let flag = flag.as_ref();
        if self.is_flag_supported(flag).unwrap_or(false) {
            self.flag(flag);
        }
        self
    }

    pub fn is_flag_supported(&self, flag: impl AsRef<OsStr>) -> Result<bool, Error> {
        let compiler = self.get_compiler();
        let temp = std::env::temp_dir().join(format!(
            "cbuild-flag-{}-{}.c",
            std::process::id(),
            flag.as_ref().to_string_lossy().chars().filter(|ch| ch.is_ascii_alphanumeric()).collect::<String>()
        ));
        std::fs::write(&temp, "int main(void){return 0;}\n").map_err(|err| Error::new(err.to_string()))?;
        let object = temp.with_extension("o");
        let mut command = Command::new(&compiler.path);
        push_compile_only(&mut command, &compiler, &object);
        command.arg(flag).arg(&temp);
        let ok = command
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_file(&object);
        Ok(ok)
    }

    pub fn warnings(&mut self, warnings: bool) -> &mut Self {
        self.warnings = warnings;
        self
    }

    pub fn extra_warnings(&mut self, _extra: bool) -> &mut Self {
        self
    }

    pub fn cargo_warnings(&mut self, _warnings: bool) -> &mut Self {
        self
    }

    pub fn cargo_metadata(&mut self, metadata: bool) -> &mut Self {
        self.cargo_metadata = metadata;
        self
    }

    pub fn cpp(&mut self, cpp: bool) -> &mut Self {
        self.cpp = cpp;
        self
    }

    pub fn std(&mut self, std: &str) -> &mut Self {
        self.std = Some(std.to_string());
        self
    }

    pub fn opt_level(&mut self, level: u32) -> &mut Self {
        self.opt_level = Some(level.to_string());
        self
    }

    pub fn opt_level_str(&mut self, level: &str) -> &mut Self {
        self.opt_level = Some(level.to_string());
        self
    }

    pub fn static_crt(&mut self, static_crt: bool) -> &mut Self {
        self.static_crt = static_crt;
        self
    }

    pub fn out_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        self.out_dir = Some(dir.as_ref().into());
        self
    }

    pub fn get_compiler(&self) -> Tool {
        self.try_get_compiler().expect("failed to find a C compiler")
    }

    pub fn try_get_compiler(&self) -> Result<Tool, Error> {
        let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
        if let Some(configured) = configured_compiler(msvc) {
            return Ok(configured);
        }
        let candidates: &[&str] = if msvc {
            &["cl", "clang-cl"]
        } else if self.cpp {
            &["c++", "g++", "clang++", "cc", "gcc", "clang"]
        } else {
            &["cc", "gcc", "clang", "cl"]
        };
        for candidate in candidates {
            if Command::new(candidate)
                .arg("--version")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
                || (msvc
                    && Command::new(candidate)
                        .output()
                        .map(|output| output.status.success() || output.status.code() == Some(0))
                        .unwrap_or(false))
            {
                return Ok(Tool {
                    path: PathBuf::from(candidate),
                    msvc: msvc || *candidate == "cl" || candidate.contains("clang-cl"),
                });
            }
        }
        Err(Error::new(
            "no C compiler found; set CC to the compiler executable",
        ))
    }

    pub fn try_expand(&self) -> Result<Vec<u8>, Error> {
        let compiler = self.try_get_compiler()?;
        let source = self
            .files
            .first()
            .ok_or_else(|| Error::new("try_expand requires a source file"))?;
        let mut command = Command::new(&compiler.path);
        if compiler.msvc {
            command.arg("/nologo").arg("/E");
        } else {
            command.arg("-E");
        }
        for include in &self.includes {
            if compiler.msvc {
                command.arg(format!("/I{}", include.display()));
            } else {
                command.arg("-I").arg(include);
            }
        }
        for (name, value) in &self.definitions {
            let flag = match value {
                Some(value) => format!("{name}={value}"),
                None => name.clone(),
            };
            command.arg(if compiler.msvc {
                format!("/D{flag}")
            } else {
                format!("-D{flag}")
            });
        }
        for flag in &self.flags {
            command.arg(map_flag(flag, compiler.msvc));
        }
        command.arg(source);
        let output = command
            .output()
            .map_err(|err| Error::new(err.to_string()))?;
        if !output.status.success() {
            return Err(Error::new(format!(
                "preprocessor failed with {}",
                output.status
            )));
        }
        Ok(output.stdout)
    }

    pub fn compile(&self, output: &str) {
        self.try_compile(output)
            .unwrap_or_else(|err| panic!("{err}"));
    }

    pub fn try_compile(&self, output: &str) -> Result<(), Error> {
        if output.contains(['/', '\\']) || output == "." || output == ".." {
            return Err(Error::new(
                "argument of `compile` must be a single normal path component",
            ));
        }
        let (lib_name, gnu_lib_name) = if let Some(stripped) = output
            .strip_prefix("lib")
            .and_then(|rest| rest.strip_suffix(".a"))
        {
            (stripped.to_string(), output.to_string())
        } else {
            (output.to_string(), format!("lib{output}.a"))
        };

        let compiler = self.try_get_compiler()?;
        let out_dir = self.out_dir.clone().unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        });
        let object_dir = out_dir.join("cbuild-objects").join(&lib_name);
        std::fs::create_dir_all(&object_dir).map_err(|err| Error::new(err.to_string()))?;

        let mut objects = Vec::with_capacity(self.files.len());
        for (index, source) in self.files.iter().enumerate() {
            println!("cargo:rerun-if-changed={}", source.display());
            let object = object_dir.join(format!("{index}.o"));
            self.compile_one(&compiler, source, &object)?;
            objects.push(object);
        }

        let archive = if compiler.msvc {
            out_dir.join(format!("{lib_name}.lib"))
        } else {
            out_dir.join(&gnu_lib_name)
        };
        archive_objects(&compiler, &archive, &objects)?;

        if self.cargo_metadata {
            println!("cargo:rustc-link-lib=static={lib_name}");
            println!("cargo:rustc-link-search=native={}", out_dir.display());
        }
        Ok(())
    }

    fn compile_one(&self, compiler: &Tool, source: &Path, object: &Path) -> Result<(), Error> {
        if compiler.msvc && source.extension().and_then(|ext| ext.to_str()) == Some("asm") {
            return assemble_masm(source, object);
        }
        let mut command = Command::new(&compiler.path);
        push_compile_only(&mut command, compiler, object);
        if compiler.msvc && self.cpp {
            command.arg("/TP");
        }
        if let Some(std) = &self.std {
            if compiler.msvc {
                let mapped = match std.as_str() {
                    "c++17" | "c++14" | "c++20" => format!("/std:{std}"),
                    "c11" | "c17" | "c99" => "/std:c11".to_string(),
                    other => format!("/std:{other}"),
                };
                command.arg(mapped);
            } else {
                command.arg(format!("-std={std}"));
            }
        }
        if compiler.msvc {
            if self.static_crt {
                command.arg("/MT");
            } else {
                command.arg("/MD");
            }
        }
        let opt = self.opt_level.clone().or_else(|| std::env::var("OPT_LEVEL").ok());
        if let Some(opt) = opt {
            command.arg(map_opt_level(&opt, compiler.msvc));
        }
        if !self.warnings {
            command.arg(if compiler.msvc { "/w" } else { "-w" });
        }
        for include in &self.includes {
            if compiler.msvc {
                command.arg(format!("/I{}", include.display()));
            } else {
                command.arg("-I").arg(include);
            }
        }
        for (name, value) in &self.definitions {
            let flag = match value {
                Some(value) => format!("{name}={value}"),
                None => name.clone(),
            };
            if compiler.msvc {
                command.arg(format!("/D{flag}"));
            } else {
                command.arg(format!("-D{flag}"));
            }
        }
        for flag in &self.flags {
            command.arg(map_flag(flag, compiler.msvc));
        }
        if !compiler.msvc && std::env::var("DEBUG").ok().as_deref() == Some("true") {
            command.arg("-g");
        }
        command.arg(source);
        run(&mut command)
    }
}

fn configured_compiler(msvc: bool) -> Option<Tool> {
    let configured = std::env::var_os("CC")?;
    if configured.is_empty() {
        return None;
    }
    let path = PathBuf::from(configured);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let msvc = msvc || name == "cl" || name == "cl.exe" || name.contains("clang-cl");
    Some(Tool { path, msvc })
}

fn push_compile_only(command: &mut Command, compiler: &Tool, object: &Path) {
    if compiler.msvc {
        command.arg("/nologo").arg("/c").arg(format!("/Fo{}", object.display()));
    } else {
        command.arg("-c").arg("-o").arg(object);
    }
}

fn map_opt_level(level: &str, msvc: bool) -> String {
    if msvc {
        match level {
            "0" => "/Od".to_string(),
            "1" => "/O1".to_string(),
            "2" | "3" => "/O2".to_string(),
            "s" | "z" => "/O1".to_string(),
            other => format!("/O{other}"),
        }
    } else {
        match level {
            "0" => "-O0".to_string(),
            "1" => "-O1".to_string(),
            "2" => "-O2".to_string(),
            "3" => "-O3".to_string(),
            "s" => "-Os".to_string(),
            "z" => "-Oz".to_string(),
            other => format!("-O{other}"),
        }
    }
}

fn map_flag(flag: &OsStr, msvc: bool) -> OsString {
    if !msvc {
        return flag.to_os_string();
    }
    let text = flag.to_string_lossy();
    if let Some(rest) = text.strip_prefix("-D") {
        return OsString::from(format!("/D{rest}"));
    }
    if let Some(rest) = text.strip_prefix("-I") {
        return OsString::from(format!("/I{rest}"));
    }
    if text == "-w" {
        return OsString::from("/w");
    }
    if let Some(rest) = text.strip_prefix("-std=") {
        return OsString::from(format!("/std:{rest}"));
    }
    if text.starts_with("-O") {
        return OsString::from(map_opt_level(&text[2..], true));
    }
    flag.to_os_string()
}

fn assemble_masm(source: &Path, object: &Path) -> Result<(), Error> {
    let assembler = std::env::var("AR_ASM").unwrap_or_else(|_| "ml64".to_string());
    let mut command = Command::new(assembler);
    command
        .arg("/nologo")
        .arg("/c")
        .arg(format!("/Fo{}", object.display()))
        .arg(source);
    run(&mut command)
}

fn archive_objects(compiler: &Tool, archive: &Path, objects: &[PathBuf]) -> Result<(), Error> {
    if objects.is_empty() {
        return Err(Error::new("no source files were given to compile"));
    }
    if compiler.msvc {
        let mut command = Command::new(std::env::var("AR").unwrap_or_else(|_| "lib".to_string()));
        command
            .arg("/nologo")
            .arg(format!("/OUT:{}", archive.display()));
        for object in objects {
            command.arg(object);
        }
        return run(&mut command);
    }
    let ar = std::env::var("AR").unwrap_or_else(|_| "ar".to_string());
    let mut first = true;
    for chunk in objects.chunks(40) {
        let mut command = Command::new(&ar);
        command.arg(if first { "rcs" } else { "r" }).arg(archive);
        first = false;
        for object in chunk {
            command.arg(object);
        }
        run(&mut command)?;
    }
    Ok(())
}

fn run(command: &mut Command) -> Result<(), Error> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .map_err(|err| Error::new(format!("failed to run {program}: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    let _ = std::io::stderr().write_all(&output.stdout);
    let _ = std::io::stderr().write_all(&output.stderr);
    Err(Error::new(format!(
        "{program} failed with {}",
        output.status
    )))
}
