use context_core::{WindowIdent, WindowRecord};

use crate::WindowTable;
use crate::apps::AppResolver;

/// The compositor window an accessible frame belongs to. The pid of the a11y connection picks
/// the candidates, or, when the toolkit talks from another process of the same app (a sandbox
/// proxy, a browser), the app the cgroup names. The frame title picks among them; without a
/// title match only a single candidate is taken, so text never lands on the wrong window.
pub(crate) fn match_window(
    windows: &WindowTable,
    resolver: &mut AppResolver,
    pid: i32,
    frame_title: &str,
) -> Option<WindowIdent> {
    let mut candidates: Vec<&WindowRecord> = windows
        .iter()
        .filter(|window| pid > 0 && window.pid == pid)
        .collect();
    if candidates.is_empty() && pid > 0 {
        let app_key = resolver.resolve("", pid).app_key;
        candidates = windows
            .iter()
            .filter(|window| {
                !app_key.is_empty()
                    && resolver.resolve(&window.app_id, window.pid).app_key == app_key
            })
            .collect();
    }
    let title = frame_title.trim();
    if let Some(window) = candidates
        .iter()
        .find(|window| !title.is_empty() && window.title.trim() == title)
    {
        return Some(window.ident.clone());
    }
    match candidates.as_slice() {
        [only] => Some(only.ident.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::DesktopIndex;

    fn window(ident: &str, pid: i32, title: &str) -> WindowRecord {
        WindowRecord {
            ident: WindowIdent(ident.into()),
            app_id: "org.example.Editor".into(),
            title: title.into(),
            pid,
            ..WindowRecord::default()
        }
    }

    fn table() -> WindowTable {
        let mut table = WindowTable::default();
        table.upsert(window("a", 10, "notes.txt"));
        table.upsert(window("b", 10, "todo.txt"));
        table.upsert(window("c", 20, "Terminal"));
        table
    }

    fn resolver() -> AppResolver {
        AppResolver::new(DesktopIndex::default()).with_proc_root("/nonexistent")
    }

    #[test]
    fn the_title_picks_among_windows_of_one_pid() {
        let ident = match_window(&table(), &mut resolver(), 10, "todo.txt");
        assert_eq!(ident, Some(WindowIdent("b".into())));
    }

    #[test]
    fn focus_does_not_settle_an_ambiguous_title() {
        let mut windows = table();
        windows.set_focus(Some(WindowIdent("a".into())));
        assert_eq!(
            match_window(&windows, &mut resolver(), 10, "Untitled"),
            None
        );
    }

    #[test]
    fn an_ambiguous_or_unknown_pid_matches_nothing() {
        assert_eq!(
            match_window(&table(), &mut resolver(), 10, "Untitled"),
            None
        );
        assert_eq!(
            match_window(&table(), &mut resolver(), 99, "notes.txt"),
            None
        );
        let ident = match_window(&table(), &mut resolver(), 20, "");
        assert_eq!(ident, Some(WindowIdent("c".into())));
    }
}
