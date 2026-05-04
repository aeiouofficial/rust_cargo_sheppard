#[cfg(windows)]
fn main() {
    let icon_path = "assets/app_icon.ico";
    println!("cargo:rerun-if-changed={icon_path}");

    if std::path::Path::new(icon_path).is_file() {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon(icon_path);
        resource
            .compile()
            .expect("failed to embed Windows application icon");
    }
}

#[cfg(not(windows))]
fn main() {}
