//! The `AppKey` of a Context API caller, from the pidfd the kernel gave at
//! accept. Sandboxed callers cannot be named over the socket yet, so they
//! get no key and no access.

use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crownos_config::schema::AppKey;
use crownos_ipc::Credentials;

/// Interpreters whose callers are named by their script, by file name prefix.
const INTERPRETERS: [&str; 12] = [
    "python", "node", "nodejs", "bun", "deno", "perl", "ruby", "bash", "sh", "dash", "zsh", "lua",
];

/// The key of the process behind `credentials`, if it still runs the
/// executable at its path and its pidfd still names it.
pub(crate) fn caller_key(credentials: &Credentials) -> Option<AppKey> {
    let pidfd = credentials.pidfd.as_ref()?;
    let pid = credentials.pid;
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let running = std::fs::metadata(format!("/proc/{pid}/exe")).ok()?;
    let on_disk = std::fs::metadata(&executable).ok()?;
    if (running.dev(), running.ino()) != (on_disk.dev(), on_disk.ino()) {
        return None;
    }
    let key = if is_interpreter(&executable) {
        AppKey::Interpreted {
            script: script_of(pid)?,
            interpreter: executable,
        }
    } else {
        AppKey::HostExecutable { path: executable }
    };
    (pid_of_pidfd(pidfd.as_raw_fd()) == Some(pid)).then_some(key)
}

fn is_interpreter(executable: &Path) -> bool {
    executable
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            INTERPRETERS.iter().any(|interpreter| {
                name.strip_prefix(interpreter).is_some_and(|version| {
                    version
                        .chars()
                        .all(|character| character.is_ascii_digit() || character == '.')
                })
            })
        })
}

fn script_of(pid: i32) -> Option<PathBuf> {
    let command_line = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let script = command_line
        .split(|byte| *byte == 0)
        .skip(1)
        .filter_map(|argument| std::str::from_utf8(argument).ok())
        .find(|argument| {
            !argument.is_empty() && !argument.starts_with('-') && *argument != "run"
        })?;
    let script = PathBuf::from(script);
    if script.is_absolute() {
        return Some(script);
    }
    let working_directory = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    Some(working_directory.join(script))
}

fn pid_of_pidfd(pidfd: i32) -> Option<i32> {
    let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{pidfd}")).ok()?;
    fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("Pid:"))
        .and_then(|pid| pid.trim().parse().ok())
        .filter(|pid: &i32| *pid > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreters_are_known_by_name_and_version() {
        assert!(is_interpreter(Path::new("/usr/bin/python3.13")));
        assert!(is_interpreter(Path::new("/usr/bin/node")));
        assert!(!is_interpreter(Path::new("/usr/bin/shotwell")));
    }
}
