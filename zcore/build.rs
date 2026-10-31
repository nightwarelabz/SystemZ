// Compile our shared freestanding C renderer. No crates or C runtime.
use std::{env, path::PathBuf, process::Command};

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("start boot renderer build tool");
    assert!(status.success(), "boot renderer build failed");
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let include = out.join("splash_art.inc");
    let object = out.join("splash.o");
    let archive = out.join("libsystemz_splash.a");
    for file in ["../boot/splash.c", "../boot/splash.h", "../boot/embed.py",
                 "../boot/assets/telepathy.rgb565"] {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-env-changed=SYSTEMZ_CC");
    println!("cargo:rerun-if-env-changed=SYSTEMZ_AR");
    run(Command::new("python3").arg("../boot/embed.py").arg(&include));
    run(Command::new(env::var("SYSTEMZ_CC").unwrap_or_else(|_| "gcc".into()))
        .args(["-std=c17", "-O2", "-Wall", "-Wextra", "-Werror",
               "-ffreestanding", "-fno-builtin", "-fno-stack-protector",
               "-fno-pie", "-fno-asynchronous-unwind-tables", "-mno-red-zone",
               "-mgeneral-regs-only", "-I../boot", "-I"])
        .arg(&out).arg("-c").arg("../boot/splash.c").arg("-o").arg(&object));
    run(Command::new(env::var("SYSTEMZ_AR").unwrap_or_else(|_| "ar".into()))
        .arg("crs").arg(&archive).arg(&object));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=systemz_splash");
}
