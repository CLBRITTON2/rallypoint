//! Embeds `assets/rallypoint.rc`, the icon Explorer shows for the executable and `watch` shows in the tray.
//!
//! Links the VC++ runtime statically and the Universal CRT, which ships with Windows, dynamically (the hybrid CRT,
//! <https://github.com/microsoft/WindowsAppSDK/blob/main/docs/Coding-Guidelines/HybridCRT.md>), so the exe runs
//! without the VC++ redistributable. A `crt-static` target feature in `.cargo/config.toml` would not do: config is
//! found from the current directory, not `--manifest-path`, and `cargo install` from crates.io ignores it.

fn main() -> Result<(), embed_resource::CompilationResult> {
    // embed-resource prints no rerun lines, and printing any stops cargo from rerunning on other changes.
    println!("cargo:rerun-if-changed=assets/rallypoint.rc");
    println!("cargo:rerun-if-changed=assets/rallypoint.ico");
    for library in ["msvcrt.lib", "vcruntime.lib", "libucrt.lib"] {
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:{library}");
    }
    for library in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
        println!("cargo:rustc-link-arg=/DEFAULTLIB:{library}");
    }
    embed_resource::compile("assets/rallypoint.rc", embed_resource::NONE).manifest_required()
}
