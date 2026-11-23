//! Searches files and database values for the test window's pixels and for image files.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use context_eval::pixel_scan::{PixelNeedles, image_signature};
use context_store::Store;
use rusqlite::types::ValueRef;

const LARGEST_FILE: u64 = 256 << 20;

#[derive(Debug, Default)]
pub(crate) struct Findings {
    pub(crate) pixel_runs: Vec<PathBuf>,
    pub(crate) images: Vec<(PathBuf, &'static str)>,
    pub(crate) files_scanned: usize,
}

impl Findings {
    pub(crate) const fn is_clean(&self) -> bool {
        self.pixel_runs.is_empty() && self.images.is_empty()
    }
}

/// What to look for, and in which files.
pub(crate) struct Search<'a> {
    pub(crate) needles: &'a PixelNeedles,
    /// Files older than this are not looked at.
    pub(crate) changed_since: SystemTime,
    /// Image signatures count only here: other programs on the machine write images too.
    pub(crate) own_directories: &'a [PathBuf],
}

impl Search<'_> {
    pub(crate) fn directory(&self, root: &Path, findings: &mut Findings) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                self.directory(&path, findings);
                continue;
            }
            let recent = metadata
                .modified()
                .is_ok_and(|modified| modified >= self.changed_since);
            if !metadata.is_file() || !recent || metadata.len() > LARGEST_FILE {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            findings.files_scanned += 1;
            if self.needles.found_in(&bytes) {
                findings.pixel_runs.push(path.clone());
            }
            let own = self.own_directories.iter().any(|own| path.starts_with(own));
            if own && let Some(format) = image_signature(&bytes) {
                findings.images.push((path, format));
            }
        }
    }
}

/// Every TEXT and BLOB value of every table, read through the key, searched for pixel runs and
/// image signatures. Returns how many values were read.
pub(crate) fn database(state_dir: &Path, needles: &PixelNeedles, findings: &mut Findings) -> usize {
    let Ok(store) = Store::open_dir(state_dir) else {
        return 0;
    };
    let connection = store.connection();
    let Ok(tables) = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
    else {
        return 0;
    };
    let mut values = 0;
    for table in tables {
        let Ok(mut statement) = connection.prepare(&format!("SELECT * FROM \"{table}\"")) else {
            continue;
        };
        let columns = statement.column_count();
        let Ok(mut rows) = statement.query([]) else {
            continue;
        };
        while let Ok(Some(row)) = rows.next() {
            for column in 0..columns {
                if let Ok(ValueRef::Text(bytes) | ValueRef::Blob(bytes)) = row.get_ref(column) {
                    values += 1;
                    let place = state_dir.join(format!("{table}[{column}]"));
                    if needles.found_in(bytes) {
                        findings.pixel_runs.push(place.clone());
                    }
                    if let Some(format) = image_signature(bytes) {
                        findings.images.push((place, format));
                    }
                }
            }
        }
    }
    values
}
