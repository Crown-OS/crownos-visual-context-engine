use std::collections::HashMap;

use context_core::AppRef;

use super::cgroup::{CgroupIdentity, parse_cgroup, unified_path};
use super::desktop::DesktopIndex;

/// Turns the app id a window reports and the pid behind it into one [`AppRef`], remembering
/// what it found per pid.
#[derive(Debug)]
pub struct AppResolver {
    desktop: DesktopIndex,
    by_pid: HashMap<i32, AppRef>,
    proc_root: std::path::PathBuf,
}

impl Default for AppResolver {
    fn default() -> Self {
        Self::new(DesktopIndex::load())
    }
}

impl AppResolver {
    pub fn new(desktop: DesktopIndex) -> Self {
        Self {
            desktop,
            by_pid: HashMap::new(),
            proc_root: std::path::PathBuf::from("/proc"),
        }
    }

    pub fn with_proc_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.proc_root = root.into();
        self
    }

    /// The app behind a window: its own app id when the desktop knows it, else what the cgroup
    /// says, else the app id as given.
    pub fn resolve(&mut self, app_id: &str, pid: i32) -> AppRef {
        if pid > 0
            && let Some(known) = self.by_pid.get(&pid)
        {
            return known.clone();
        }
        let cgroup = (pid > 0)
            .then(|| std::fs::read_to_string(self.proc_root.join(pid.to_string()).join("cgroup")))
            .and_then(Result::ok)
            .map(|text| parse_cgroup(unified_path(&text)));
        let resolved = self.identify(app_id, cgroup.as_ref());
        if pid > 0 {
            self.by_pid.insert(pid, resolved.clone());
        }
        resolved
    }

    pub fn forget_pid(&mut self, pid: i32) {
        self.by_pid.remove(&pid);
    }

    fn identify(&self, app_id: &str, cgroup: Option<&CgroupIdentity>) -> AppRef {
        let cgroup_id = match cgroup {
            Some(
                CgroupIdentity::App(id) | CgroupIdentity::Flatpak(id) | CgroupIdentity::Snap(id),
            ) => Some(id.as_str()),
            Some(CgroupIdentity::Terminal(name)) => Some(name.as_str()),
            _ => None,
        };
        let candidates = [Some(app_id).filter(|id| !id.is_empty()), cgroup_id];
        for candidate in candidates.into_iter().flatten() {
            if let Some(entry) = self.desktop.lookup(candidate) {
                return AppRef {
                    app_key: entry.id.to_ascii_lowercase(),
                    app_id: app_id.to_owned(),
                    name: entry.name.clone(),
                };
            }
        }
        let key = cgroup_id.filter(|_| app_id.is_empty()).unwrap_or(app_id);
        AppRef {
            app_key: key.to_ascii_lowercase(),
            app_id: app_id.to_owned(),
            name: key.rsplit('.').next().unwrap_or(key).to_owned(),
        }
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
    use crate::apps::DesktopEntry;

    fn resolver() -> AppResolver {
        let mut desktop = DesktopIndex::default();
        desktop.insert(
            DesktopEntry::parse(
                "org.mozilla.firefox",
                "[Desktop Entry]\nName=Firefox\nStartupWMClass=firefox\n",
            )
            .unwrap(),
        );
        AppResolver::new(desktop)
    }

    #[test]
    fn a_known_app_id_takes_its_desktop_name() {
        let mut resolver = resolver();
        let app = resolver.resolve("firefox", 0);
        assert_eq!(app.app_key, "org.mozilla.firefox");
        assert_eq!(app.name, "Firefox");
    }

    #[test]
    fn an_unknown_app_keeps_its_id_and_a_short_name() {
        let mut resolver = resolver();
        let app = resolver.resolve("org.kde.Dolphin", 0);
        assert_eq!(app.app_key, "org.kde.dolphin");
        assert_eq!(app.name, "Dolphin");
    }

    #[test]
    fn the_cgroup_names_an_app_without_an_app_id() {
        let dir = std::env::temp_dir().join(format!("context-collect-proc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("4202")).unwrap();
        std::fs::write(
            dir.join("4202/cgroup"),
            "0::/user.slice/app.slice/app-gnome-org.mozilla.firefox-4202.scope\n",
        )
        .unwrap();
        let mut resolver = resolver().with_proc_root(&dir);
        let app = resolver.resolve("", 4202);
        assert_eq!(app.app_key, "org.mozilla.firefox");
        assert_eq!(resolver.resolve("", 4202), app);
        let _ = std::fs::remove_dir_all(dir);
    }
}
