use std::path::PathBuf;

/// `$XDG_CACHE_HOME/crownos/context/ort/<provider>`: compiled engines and blobs, so a provider
/// compiles a graph once, not at each start.
pub(crate) fn cache_dir(provider: &str) -> Option<String> {
    let directory: PathBuf = dirs::cache_dir()?
        .join("crownos")
        .join("context")
        .join("ort")
        .join(provider);
    std::fs::create_dir_all(&directory).ok()?;
    directory.into_os_string().into_string().ok()
}
