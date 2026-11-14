use std::path::PathBuf;

/// `$XDG_STATE_HOME/crownos/context`, or its default under the home directory.
pub(crate) fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("crownos").join("context")
}
