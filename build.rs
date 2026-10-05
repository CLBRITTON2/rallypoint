//! Embeds `assets/rallypoint.rc`, the icon Explorer shows for the executable and `watch` shows in the tray.

fn main() -> Result<(), embed_resource::CompilationResult> {
    // embed-resource prints no rerun lines, and printing any stops cargo from rerunning on other changes.
    println!("cargo:rerun-if-changed=assets/rallypoint.rc");
    println!("cargo:rerun-if-changed=assets/rallypoint.ico");
    embed_resource::compile("assets/rallypoint.rc", embed_resource::NONE).manifest_required()
}
