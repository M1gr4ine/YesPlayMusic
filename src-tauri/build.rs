use std::{env, path::PathBuf, process::Command};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        compile_taskbar_helper();
    }
    tauri_build::build();
}

/// Builds the Windows taskbar helper with the .NET Framework compiler that
/// ships with Windows; `tauri.windows.conf.json` bundles the result.
fn compile_taskbar_helper() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest
        .join("..")
        .join("scripts")
        .join("windows-taskbar-native.cs");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-env-changed=WINDIR");

    let windows = env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let compiler = ["Framework64", "Framework"]
        .map(|framework| windows.join(format!(r"Microsoft.NET\{framework}\v4.0.30319\csc.exe")))
        .into_iter()
        .find(|path| path.is_file())
        .expect("the .NET Framework C# compiler is required to build on Windows");

    let output = manifest.join("resources/windows-taskbar-native.exe");
    std::fs::create_dir_all(output.parent().expect("resource directory"))
        .expect("create the resource directory");
    let result = Command::new(&compiler)
        .args(["/nologo", "/target:exe", "/platform:x64", "/optimize+"])
        .arg(format!("/out:{}", output.display()))
        .args([
            "/reference:Accessibility.dll",
            "/reference:System.Web.Extensions.dll",
        ])
        .arg(&source)
        .output()
        .expect("start the C# compiler");
    assert!(
        result.status.success(),
        "the taskbar helper failed to compile: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!(
        "cargo:rustc-env=TASKBAR_NATIVE_HELPER_PATH={}",
        output.display()
    );
}
