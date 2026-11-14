use std::collections::BTreeMap;

use context_core::{WindowIdent, WindowRecord};

/// Every window the compositor has described, by identifier.
#[derive(Debug, Default)]
pub struct WindowTable {
    windows: BTreeMap<WindowIdent, WindowRecord>,
    focus: Option<WindowIdent>,
}

impl WindowTable {
    pub fn upsert(&mut self, record: WindowRecord) -> bool {
        let changed = self.windows.get(&record.ident) != Some(&record);
        self.windows.insert(record.ident.clone(), record);
        changed
    }

    pub fn remove(&mut self, ident: &WindowIdent) -> Option<WindowRecord> {
        if self.focus.as_ref() == Some(ident) {
            self.focus = None;
        }
        self.windows.remove(ident)
    }

    pub fn set_focus(&mut self, ident: Option<WindowIdent>) {
        self.focus = ident;
    }

    pub fn focus(&self) -> Option<&WindowRecord> {
        self.focus
            .as_ref()
            .and_then(|ident| self.windows.get(ident))
    }

    pub fn get(&self, ident: &WindowIdent) -> Option<&WindowRecord> {
        self.windows.get(ident)
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &WindowRecord> {
        self.windows.values()
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

    fn window(ident: &str, title: &str) -> WindowRecord {
        WindowRecord {
            ident: WindowIdent(ident.into()),
            title: title.into(),
            ..WindowRecord::default()
        }
    }

    #[test]
    fn focus_follows_the_window_out() {
        let mut table = WindowTable::default();
        assert!(table.upsert(window("a", "one")));
        assert!(!table.upsert(window("a", "one")));
        assert!(table.upsert(window("a", "two")));
        table.set_focus(Some(WindowIdent("a".into())));
        assert_eq!(table.focus().map(|w| w.title.as_str()), Some("two"));
        table.remove(&WindowIdent("a".into()));
        assert!(table.focus().is_none());
        assert!(table.is_empty());
    }
}
