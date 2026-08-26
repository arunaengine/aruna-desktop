use std::path::{Path, PathBuf};

use crate::error::DesktopError;
use crate::settings::{NODE_BIN_ENV, NodeSettings};

#[cfg(windows)]
const SIDECAR_NAME: &str = "aruna.exe";
#[cfg(not(windows))]
const SIDECAR_NAME: &str = "aruna";

/// What `build.rs` writes in the sidecar's place when none was synced; the same
/// text, so the two cannot drift apart unnoticed.
const PLACEHOLDER_MARK: &str = "# aruna-desktop placeholder";
const SYNC_HINT: &str = "run scripts/sync-node.sh to install the node";

/// The app stays usable without a node binary, so a miss reports every place it
/// looked instead of failing silently.
pub fn resolve(settings: &NodeSettings, resources: Option<&Path>) -> Result<PathBuf, DesktopError> {
    let dev = std::env::var_os(NODE_BIN_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    pick(candidates(dev, settings, resources))
}

/// Where the node binary may live, most specific first: the dev override, the
/// configured path, the sidecar next to this app, then the resource directory.
fn candidates(
    dev: Option<PathBuf>,
    settings: &NodeSettings,
    resources: Option<&Path>,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    found.extend(dev);
    found.extend(settings.binary_path.clone());
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        found.push(dir.join(SIDECAR_NAME));
    }
    if let Some(dir) = resources {
        found.push(dir.join("binaries").join(SIDECAR_NAME));
    }
    found
}

fn pick(candidates: Vec<PathBuf>) -> Result<PathBuf, DesktopError> {
    let mut placeholder = false;
    for path in &candidates {
        if !path.is_file() {
            continue;
        }
        if is_placeholder(path) {
            placeholder = true;
            continue;
        }
        return Ok(path.clone());
    }
    let mut searched = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if placeholder {
        searched = format!("{searched} ({SYNC_HINT})");
    }
    Err(DesktopError::BinaryMissing(searched))
}

/// A build without a synced node carries a stand-in that only names the sync
/// script; running it would report a failed node instead of a missing one.
fn is_placeholder(path: &Path) -> bool {
    let mut head = [0u8; 128];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(read) = std::io::Read::read(&mut file, &mut head) else {
        return false;
    };
    String::from_utf8_lossy(&head[..read]).contains(PLACEHOLDER_MARK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_candidates() {
        let settings = NodeSettings {
            binary_path: Some(PathBuf::from("/opt/aruna/node")),
            ..NodeSettings::default()
        };
        let found = candidates(
            Some(PathBuf::from("/dev/override")),
            &settings,
            Some(Path::new("/res")),
        );
        assert_eq!(found[0], PathBuf::from("/dev/override"));
        assert_eq!(found[1], PathBuf::from("/opt/aruna/node"));
        assert_eq!(
            found.last(),
            Some(&Path::new("/res").join("binaries").join(SIDECAR_NAME))
        );
    }

    #[test]
    fn reports_searched_paths() {
        let error = pick(vec![PathBuf::from("/nonexistent/aruna")]).unwrap_err();
        assert!(error.to_string().contains("/nonexistent/aruna"));
        assert!(!error.to_string().contains(SYNC_HINT));
    }

    #[test]
    fn skips_the_placeholder() {
        // The stand-in build.rs writes is named, never run; a real file after
        // it still wins.
        let dir =
            std::env::temp_dir().join(format!("aruna-desktop-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stand_in = dir.join("aruna");
        std::fs::write(
            &stand_in,
            format!("#!/bin/sh\n{PLACEHOLDER_MARK}: x\nexit 1\n"),
        )
        .unwrap();
        let real = dir.join("aruna-real");
        std::fs::write(&real, b"ELF").unwrap();

        let error = pick(vec![stand_in.clone()]).unwrap_err();
        assert!(error.to_string().contains(SYNC_HINT));
        assert_eq!(pick(vec![stand_in, real.clone()]).unwrap(), real);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
