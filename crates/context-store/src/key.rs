use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const KEY_FILE: &str = "key";
const KEY_LEN: usize = 32;
const OWNER_ONLY: u32 = 0o600;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("key file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("key file {0} is not {KEY_LEN} bytes of hex")]
    Corrupt(PathBuf),
    #[error("no randomness for a new key: {0}")]
    Random(#[from] getrandom::Error),
    #[error("key file {0} is readable by others")]
    Exposed(PathBuf),
}

/// The database key as SQLCipher wants it: a raw key in hex. Created on first run, readable by
/// the owner only, refused when anyone else could read it.
///
/// # Errors
///
/// Fails on an unreadable, corrupt or exposed key file, or when a new one cannot be written.
pub fn load_or_create_key(state_dir: &Path) -> Result<String, KeyError> {
    let path = state_dir.join(KEY_FILE);
    let io = |source| KeyError::Io {
        path: path.clone(),
        source,
    };
    fs::create_dir_all(state_dir).map_err(io)?;
    match fs::read_to_string(&path) {
        Ok(text) => {
            let mode = fs::metadata(&path).map_err(io)?.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(KeyError::Exposed(path));
            }
            let hex = text.trim();
            let valid =
                hex.len() == KEY_LEN * 2 && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
            valid.then(|| hex.to_owned()).ok_or(KeyError::Corrupt(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut bytes = [0u8; KEY_LEN];
            getrandom::fill(&mut bytes)?;
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(OWNER_ONLY)
                .open(&path)
                .map_err(io)?;
            file.write_all(hex.as_bytes()).map_err(io)?;
            file.write_all(b"\n").map_err(io)?;
            Ok(hex)
        }
        Err(source) => Err(io(source)),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("context-store-key-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_key_is_made_once_and_read_back() {
        let dir = scratch("once");
        let first = load_or_create_key(&dir).unwrap();
        let second = load_or_create_key(&dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        let mode = fs::metadata(dir.join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exposed_or_corrupt_keys_are_refused() {
        let dir = scratch("exposed");
        load_or_create_key(&dir).unwrap();
        fs::set_permissions(dir.join(KEY_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            load_or_create_key(&dir),
            Err(KeyError::Exposed(_))
        ));
        fs::set_permissions(dir.join(KEY_FILE), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(dir.join(KEY_FILE), "not hex\n").unwrap();
        assert!(matches!(
            load_or_create_key(&dir),
            Err(KeyError::Corrupt(_))
        ));
        let _ = fs::remove_dir_all(dir);
    }
}
