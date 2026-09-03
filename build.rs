use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=c/ffbwrapper.c");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let x86_64_target = out_dir.join("libffbwrapper-x86_64.so");
    let i386_target = out_dir.join("libffbwrapper-i386.so");

    // 1. Compile x86_64 library
    let status_64 = Command::new("gcc")
        .args([
            "-Wall",
            "-Wextra",
            "-fPIC",
            "-shared",
            "c/ffbwrapper.c",
            "-o",
            x86_64_target.to_str().unwrap(),
            "-lrt",
            "-ldl",
        ])
        .status();

    match status_64 {
        Ok(s) if s.success() => {}
        _ => panic!("Failed to compile x86_64 ffbwrapper interceptor via gcc"),
    }

    // 2. Compile i386 library (if 32-bit headers exist on system, else write empty file)
    let status_32 = Command::new("gcc")
        .args([
            "-Wall",
            "-Wextra",
            "-m32",
            "-fPIC",
            "-shared",
            "c/ffbwrapper.c",
            "-o",
            i386_target.to_str().unwrap(),
            "-lrt",
            "-ldl",
        ])
        .status();

    if let Ok(s) = status_32 {
        if !s.success() {
            let _ = fs::write(&i386_target, []);
        }
    } else {
        let _ = fs::write(&i386_target, []);
    }
}
