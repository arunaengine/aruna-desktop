use std::path::{Path, PathBuf};

use crate::error::DesktopError;
use crate::settings::{NODE_BIN_ENV, NodeSettings};

#[cfg(windows)]
const SIDECAR_NAME: &str = "aruna.exe";
#[cfg(not(windows))]
const SIDECAR_NAME: &str = "aruna";

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
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }
    let searched = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(DesktopError::BinaryMissing(searched))
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
    }
}
