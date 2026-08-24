use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/fallback.html");
    place_fallback(Path::new("../ui"));
    tauri_build::build()
}

/// The window serves the portal artifact from `ui/`, which `scripts/sync-frontend.sh`
/// installs from the website repository. A checkout without one still has to
/// build, so a page naming that command is written in its place.
fn place_fallback(dir: &Path) {
    let index = dir.join("index.html");
    if index.exists() {
        return;
    }
    std::fs::create_dir_all(dir).expect("the portal directory is writable");
    std::fs::write(index, include_str!("src/fallback.html")).expect("the fallback page is written");
}
