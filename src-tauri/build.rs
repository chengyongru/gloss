fn main() {
    // Keep Tauri's icon and version resource, but link the application manifest separately so
    // GNU unit-test executables receive Common Controls v6 as well as the desktop binary.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attributes).expect("failed to prepare the Tauri build");

    #[cfg(windows)]
    let _ = embed_resource::compile_for_everything("windows-app-manifest.rc", embed_resource::NONE);
}
