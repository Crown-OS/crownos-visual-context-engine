//! inferd serves crown-contextd only. It reads the peer's program when it accepts the
//! connection, so contextd connects before it turns off `PR_SET_DUMPABLE`.

use std::path::{Path, PathBuf};

use crownos_config::schema::{AppKey, CONTEXT_ENGINE_EXECUTABLE};
use crownos_ipc::{Authorizer, Credentials};

/// A development build of contextd, or a test, to serve as well. Debug builds only.
#[cfg(debug_assertions)]
const ENGINE_OVERRIDE: &str = "CROWN_CONTEXT_ENGINE";

/// Accepts a peer of this user that runs the root-owned `/usr/bin/crown-contextd`, or, in a
/// debug build, the program named in `CROWN_CONTEXT_ENGINE`.
pub(crate) fn authorizer() -> Authorizer {
    let development = development_engine();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Authorizer::Custom(Box::new(move |credentials| {
        let accepted = is_engine(credentials, development.as_deref(), home.as_deref());
        if !accepted {
            tracing::warn!(
                pid = credentials.pid,
                "refused a peer that is not crown-contextd"
            );
        }
        accepted
    }))
}

fn development_engine() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os(ENGINE_OVERRIDE) {
        return Some(PathBuf::from(path));
    }
    None
}

fn is_engine(credentials: &Credentials, development: Option<&Path>, home: Option<&Path>) -> bool {
    if credentials.uid != rustix::process::geteuid().as_raw() {
        return false;
    }
    let Some(program) = &credentials.program else {
        return false;
    };
    let key = AppKey::of_program(
        &program.executable,
        &program.arguments,
        program.working_directory.as_deref(),
    );
    match &key {
        Some(engine @ AppKey::HostExecutable { path }) => {
            development.is_some_and(|development| path == development)
                || (path == Path::new(CONTEXT_ENGINE_EXECUTABLE) && !engine.is_session_only(home))
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::*;

    #[test]
    fn only_the_named_engine_is_served() {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let credentials = Credentials::from_socket(&ours).unwrap();
        let this_test = std::env::current_exe().unwrap();
        assert!(is_engine(&credentials, Some(&this_test), None));
        assert!(!is_engine(&credentials, None, None));
        assert!(!is_engine(
            &credentials,
            Some(Path::new("/usr/bin/other")),
            None
        ));
    }
}
