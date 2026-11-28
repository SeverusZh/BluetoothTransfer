//! 仅在 Windows 原生构建时编译图标与版本资源；交叉类型检查跳过。

#[cfg(windows)]
fn compile_windows_resources() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let icon = manifest
        .join("..")
        .join("..")
        .join("packaging")
        .join("windows")
        .join("app.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let icon = match icon.to_str() {
        Some(path) => path,
        None => panic!("图标路径不是有效 UTF-8：{}", icon.display()),
    };
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon);
    resource.set("FileDescription", "Bluetooth Transfer");
    resource.set("ProductName", "Bluetooth Transfer");
    resource.set("OriginalFilename", "bt-gui.exe");
    resource.set("LegalCopyright", "MIT License");
    if let Err(error) = resource.compile() {
        panic!("编译 Windows 资源失败：{error}");
    }
}

#[cfg(not(windows))]
fn compile_windows_resources() {}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    compile_windows_resources();
}
