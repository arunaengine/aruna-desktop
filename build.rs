use std::path::Path;

/// Marks the stand-in node `place_node_placeholder` writes; `src/node/binary.rs`
/// recognises the same text and reports it instead of running it.
const NODE_PLACEHOLDER: &str = "#!/bin/sh\n# aruna-desktop placeholder: run scripts/sync-node.sh to install the node\nexit 1\n";

fn main() {
    println!("cargo:rerun-if-changed=src/fallback.html");
    place_fallback(Path::new("ui"));
    place_node_placeholder(Path::new("binaries"));
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

/// The node is the Tauri sidecar `binaries/aruna-<target>`, which
/// `scripts/sync-node.sh` installs. tauri-build refuses to build without that
/// file, so a checkout that never synced one gets a placeholder the app names.
fn place_node_placeholder(dir: &Path) {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let path = dir.join(format!("aruna-{target}{suffix}"));
    println!("cargo:rerun-if-changed={}", path.display());
    if path.exists() {
        return;
    }
    std::fs::create_dir_all(dir).expect("the binaries directory is writable");
    std::fs::write(&path, NODE_PLACEHOLDER).expect("the node placeholder is written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
}
