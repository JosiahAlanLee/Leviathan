#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result<()> {
    leviathan_app::run_desktop()
}

// cargo-apk runs the NativeActivity entry point in the library, not this binary.
#[cfg(target_os = "android")]
fn main() {}
