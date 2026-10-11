use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=../scripts/windows-taskbar-native.cs");
    println!("cargo:rerun-if-env-changed=WINDIR");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        compile_windows_helper();
    }
    tauri_build::build();
}

fn compile_windows_helper() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest
        .join("..")
        .join("scripts")
        .join("windows-taskbar-native.cs");
    if !source.is_file() {
        panic!(
            "Windows taskbar helper source is missing: {}",
            source.display()
        );
    }

    let windows_dir = env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let compiler = [
        windows_dir.join(r"Microsoft.NET\Framework64\v4.0.30319\csc.exe"),
        windows_dir.join(r"Microsoft.NET\Framework\v4.0.30319\csc.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .unwrap_or_else(|| {
        panic!(
            "Could not find the .NET Framework C# compiler under {}",
            windows_dir.display()
        )
    });

    let resources = manifest.join("resources");
    std::fs::create_dir_all(&resources).expect("create Tauri resource directory");
    let output = resources.join("windows-taskbar-native.exe");
    let compiler_output = Command::new(&compiler)
        .arg("/nologo")
        .arg("/target:exe")
        .arg("/platform:x64")
        .arg("/optimize+")
        .arg(format!("/out:{}", output.display()))
        .arg("/reference:Accessibility.dll")
        .arg("/reference:System.Web.Extensions.dll")
        .arg(&source)
        .output()
        .unwrap_or_else(|error| panic!("Could not start {}: {error}", compiler.display()));
    if !compiler_output.status.success() {
        panic!(
            "C# compiler failed to build the Windows taskbar helper ({}): {}{}",
            compiler_output.status,
            String::from_utf8_lossy(&compiler_output.stdout),
            String::from_utf8_lossy(&compiler_output.stderr)
        );
    }
    println!(
        "cargo:rustc-env=TASKBAR_NATIVE_HELPER_PATH={}",
        output.display()
    );
}
