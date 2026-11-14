use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The parts of a `.desktop` file the engine files an app by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub startup_wm_class: Option<String>,
}

impl DesktopEntry {
    pub fn parse(id: &str, text: &str) -> Option<Self> {
        let mut in_entry = false;
        let mut name = None;
        let mut categories = Vec::new();
        let mut startup_wm_class = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "Name" => name = Some(value.trim().to_owned()),
                "Categories" => {
                    categories = value
                        .split(';')
                        .filter(|category| !category.is_empty())
                        .map(str::to_owned)
                        .collect()
                }
                "StartupWMClass" => startup_wm_class = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        Some(Self {
            id: id.to_owned(),
            name: name?,
            categories,
            startup_wm_class,
        })
    }
}

/// Every desktop entry on the XDG data path, by id and by `StartupWMClass`.
#[derive(Debug, Default)]
pub struct DesktopIndex {
    by_id: HashMap<String, DesktopEntry>,
    by_wm_class: HashMap<String, String>,
}

impl DesktopIndex {
    /// Reads `$XDG_DATA_HOME/applications` and every `$XDG_DATA_DIRS/applications`.
    pub fn load() -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(home) = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
        {
            dirs.push(home.join("applications"));
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
        dirs.extend(
            data_dirs
                .split(':')
                .map(|dir| Path::new(dir).join("applications")),
        );
        let mut index = Self::default();
        for dir in dirs {
            index.scan(&dir);
        }
        index
    }

    pub fn scan(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".desktop"))
            else {
                continue;
            };
            if self.by_id.contains_key(id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(parsed) = DesktopEntry::parse(id, &text) {
                if let Some(class) = &parsed.startup_wm_class {
                    self.by_wm_class
                        .insert(class.to_ascii_lowercase(), id.to_owned());
                }
                self.by_id.insert(id.to_owned(), parsed);
            }
        }
    }

    pub fn insert(&mut self, entry: DesktopEntry) {
        if let Some(class) = &entry.startup_wm_class {
            self.by_wm_class
                .insert(class.to_ascii_lowercase(), entry.id.clone());
        }
        self.by_id.insert(entry.id.clone(), entry);
    }

    /// The entry for an app id, exactly, by case-insensitive id, or by `StartupWMClass`.
    pub fn lookup(&self, app_id: &str) -> Option<&DesktopEntry> {
        if let Some(entry) = self.by_id.get(app_id) {
            return Some(entry);
        }
        let lower = app_id.to_ascii_lowercase();
        self.by_id
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(app_id))
            .map(|(_, entry)| entry)
            .or_else(|| {
                self.by_wm_class
                    .get(&lower)
                    .and_then(|id| self.by_id.get(id))
            })
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

    const FIREFOX: &str = "[Desktop Entry]\nName=Firefox\nExec=firefox %u\nCategories=Network;WebBrowser;\nStartupWMClass=firefox\n\n[Desktop Action new-window]\nName=New Window\n";

    #[test]
    fn entries_parse_their_main_group_only() {
        let entry = DesktopEntry::parse("org.mozilla.firefox", FIREFOX).unwrap();
        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.categories, vec!["Network", "WebBrowser"]);
        assert_eq!(entry.startup_wm_class.as_deref(), Some("firefox"));
    }

    #[test]
    fn lookups_fall_back_to_case_and_wm_class() {
        let mut index = DesktopIndex::default();
        index.insert(DesktopEntry::parse("org.mozilla.firefox", FIREFOX).unwrap());
        assert!(index.lookup("org.mozilla.firefox").is_some());
        assert!(index.lookup("ORG.mozilla.Firefox").is_some());
        assert_eq!(
            index.lookup("firefox").map(|e| e.id.as_str()),
            Some("org.mozilla.firefox")
        );
        assert!(index.lookup("kitty").is_none());
    }
}
