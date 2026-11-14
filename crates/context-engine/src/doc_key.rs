//! What a window is about, from the little it tells: a title, maybe a URL, maybe a directory.

use std::path::{Path, PathBuf};

use context_core::{DocKey, WindowRecord};
use url::Url;

const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "fbclid",
    "gclid",
    "mc_cid",
    "mc_eid",
    "ref_src",
    "igshid",
];

const APP_SUFFIXES: &[&str] = &[
    "Mozilla Firefox",
    "Firefox",
    "Chromium",
    "Google Chrome",
    "Brave",
    "Vivaldi",
    "Zen Browser",
    "Code - OSS",
    "Visual Studio Code",
    "Mozilla Thunderbird",
    "Slack",
    "Discord",
    "LibreOffice Writer",
    "LibreOffice Calc",
    "Zed",
    "Obsidian",
];

/// A title without the parts that change while the document does not: unread counts, dirty
/// markers, the app's own name.
pub fn normalize_title(title: &str) -> String {
    let mut text = title.trim();
    loop {
        let before = text;
        text = text.trim_start_matches(['•', '*', '●']).trim_start();
        if let Some(rest) = text.strip_prefix('(')
            && let Some((count, after)) = rest.split_once(')')
            && count.bytes().all(|byte| byte.is_ascii_digit())
        {
            text = after.trim_start();
        }
        if text == before {
            break;
        }
    }
    let mut owned = text.to_owned();
    for separator in [" — ", " - ", " – ", " | "] {
        for suffix in APP_SUFFIXES {
            let tail = format!("{separator}{suffix}");
            if let Some(stripped) = owned.strip_suffix(tail.as_str()) {
                owned = stripped.to_owned();
            }
        }
    }
    owned = owned.trim_end_matches([' ', '*', '•']).to_owned();
    owned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A URL with the parts that do not name the page removed.
pub fn canonical_url(text: &str) -> Option<String> {
    let mut url = Url::parse(text.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "file") {
        return None;
    }
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !TRACKING_PARAMS.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (key, value) in &kept {
            pairs.append_pair(key, value);
        }
    }
    let mut canonical = url.to_string();
    if url.path() != "/" && canonical.ends_with('/') {
        canonical.pop();
    }
    Some(canonical)
}

/// The nearest ancestor of `path` holding a `.git`, the unit a project is.
pub fn git_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The working directory a terminal shows in its title: `~/crownpositor — fish`,
/// `user@host: /srv/www`, `/etc (3)`.
pub fn terminal_directory(title: &str, home: Option<&str>) -> Option<String> {
    let head = title
        .split([' ', '—', '-', ':'])
        .map(str::trim)
        .find(|part| part.starts_with('~') || part.starts_with('/'))?;
    let path = match (head.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => format!("{home}{rest}"),
        (Some(_), None) => return None,
        (None, _) => head.to_owned(),
    };
    Some(path.trim_end_matches('/').to_owned()).filter(|p| !p.is_empty())
}

/// The key a segment is filed under: the first known of document URL, file, directory, title.
pub fn derive_doc_key(window: &WindowRecord, url: Option<&str>, home: Option<&str>) -> DocKey {
    if let Some(url) = url.and_then(canonical_url) {
        return DocKey::Url(url);
    }
    if is_terminal(&window.app_id)
        && let Some(directory) = terminal_directory(&window.title, home)
    {
        let root = git_root(Path::new(&directory))
            .map(|root| root.to_string_lossy().into_owned())
            .unwrap_or(directory);
        return DocKey::Directory(root);
    }
    DocKey::Title(normalize_title(&window.title))
}

fn is_terminal(app_id: &str) -> bool {
    let lower = app_id.to_ascii_lowercase();
    [
        "kitty",
        "alacritty",
        "foot",
        "konsole",
        "terminal",
        "wezterm",
        "ghostty",
        "tilix",
    ]
    .iter()
    .any(|name| lower.contains(name))
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

    #[test]
    fn titles_lose_counters_markers_and_app_names() {
        assert_eq!(normalize_title("(3) Inbox — Mozilla Thunderbird"), "inbox");
        assert_eq!(
            normalize_title("● mod.rs — crownpositor — Code - OSS"),
            "mod.rs — crownpositor"
        );
        assert_eq!(
            normalize_title("niri pw_utils.rs at main · YaLTeR/niri — Mozilla Firefox"),
            "niri pw_utils.rs at main · yalter/niri"
        );
        assert_eq!(normalize_title("  Notes*  "), "notes");
    }

    #[test]
    fn urls_drop_tracking_fragments_and_trailing_slashes() {
        assert_eq!(
            canonical_url("https://Docs.rs/smithay/latest/?utm_source=x&page=2#frag").unwrap(),
            "https://docs.rs/smithay/latest/?page=2"
        );
        assert_eq!(
            canonical_url("https://github.com/YaLTeR/niri/").unwrap(),
            "https://github.com/YaLTeR/niri"
        );
        assert_eq!(
            canonical_url("https://example.com/").unwrap(),
            "https://example.com/"
        );
        assert!(canonical_url("mailto:a@b.c").is_none());
        assert!(canonical_url("not a url").is_none());
    }

    #[test]
    fn terminal_titles_name_their_directory() {
        assert_eq!(
            terminal_directory("~/crownpositor — fish", Some("/home/u")).as_deref(),
            Some("/home/u/crownpositor")
        );
        assert_eq!(
            terminal_directory("u@arch: /srv/www", None).as_deref(),
            Some("/srv/www")
        );
        assert_eq!(terminal_directory("fish", None), None);
        assert_eq!(terminal_directory("~/x", None), None);
    }

    #[test]
    fn git_roots_are_found_upward() {
        let dir = std::env::temp_dir().join(format!("context-engine-git-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.join("repo/src/deep")).unwrap();
        assert_eq!(git_root(&dir.join("repo/src/deep")), Some(dir.join("repo")));
        assert_eq!(git_root(Path::new("/nonexistent/x")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn doc_keys_prefer_url_then_directory_then_title() {
        let mut window = WindowRecord {
            app_id: "kitty".into(),
            title: "~/proj — fish".into(),
            ..WindowRecord::default()
        };
        assert_eq!(
            derive_doc_key(&window, None, Some("/home/u")),
            DocKey::Directory("/home/u/proj".into())
        );
        assert_eq!(
            derive_doc_key(&window, Some("https://a.b/c/"), None),
            DocKey::Url("https://a.b/c".into())
        );
        window.app_id = "code-oss".into();
        window.title = "(2) lib.rs — Code - OSS".into();
        assert_eq!(
            derive_doc_key(&window, None, None),
            DocKey::Title("lib.rs".into())
        );
    }
}
