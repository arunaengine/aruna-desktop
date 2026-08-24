use serde::{Serialize, Serializer};

/// Every command failure the frontend can see. Serialized as its message, which
/// is what the views render.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("no node binary found; searched: {0}")]
    BinaryMissing(String),
    #[error("invalid settings: {0}")]
    Settings(String),
    #[error("the node could not be started: {0}")]
    Spawn(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

impl Serialize for DesktopError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
