//! The gate every observation passes.

use context_core::WindowRecord;
use regex::{Regex, RegexSet};

use crate::config::VisualContext;
use crate::util::glob;

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("private title pattern {pattern:?}: {source}")]
    Pattern {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Disabled,
    ExcludedApp,
    PrivateBrowsing,
    PasswordInput,
    AppRequested,
    EngineFlagged,
    MotionContent,
}

/// How much of a window the engine may take in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Metadata, text and captures.
    Observe,
    /// Focus and timing only: no text, no captures.
    MetadataOnly(Reason),
    /// Nothing at all, not even that the window exists.
    Drop(Reason),
}

impl Verdict {
    pub const fn allows_text(self) -> bool {
        matches!(self, Self::Observe)
    }

    pub const fn allows_metadata(self) -> bool {
        !matches!(self, Self::Drop(_))
    }
}

#[derive(Debug)]
pub struct Policy {
    enabled: bool,
    excluded_apps: Vec<String>,
    private_titles: RegexSet,
    clipboard_apps: Vec<String>,
}

impl Policy {
    /// # Errors
    ///
    /// Fails when a title pattern is not a regular expression.
    pub fn from_section(section: &VisualContext) -> Result<Self, PolicyError> {
        for pattern in &section.private_titles {
            Regex::new(pattern).map_err(|source| PolicyError::Pattern {
                pattern: pattern.clone(),
                source,
            })?;
        }
        let private_titles =
            RegexSet::new(&section.private_titles).map_err(|source| PolicyError::Pattern {
                pattern: String::new(),
                source,
            })?;
        Ok(Self {
            enabled: section.enabled,
            excluded_apps: section.excluded_apps.clone(),
            private_titles,
            clipboard_apps: section.clipboard_apps.clone(),
        })
    }

    pub fn judge(&self, window: &WindowRecord) -> Verdict {
        if !self.enabled {
            return Verdict::Drop(Reason::Disabled);
        }
        if window.sensitivity.excluded || self.is_excluded(&window.app_id) {
            return Verdict::Drop(Reason::ExcludedApp);
        }
        if window.sensitivity.private_browsing || self.private_titles.is_match(&window.title) {
            return Verdict::Drop(Reason::PrivateBrowsing);
        }
        if window.sensitivity.app_requested {
            return Verdict::MetadataOnly(Reason::AppRequested);
        }
        if window.sensitivity.password_input {
            return Verdict::MetadataOnly(Reason::PasswordInput);
        }
        if window.sensitivity.engine_flagged {
            return Verdict::MetadataOnly(Reason::EngineFlagged);
        }
        if window.content_type.is_motion() {
            return Verdict::MetadataOnly(Reason::MotionContent);
        }
        Verdict::Observe
    }

    pub fn is_excluded(&self, app_id: &str) -> bool {
        self.excluded_apps
            .iter()
            .any(|pattern| glob::matches(pattern, app_id))
    }

    /// Whether clipboard content, not only its metadata, may be read from `app_id`.
    pub fn clipboard_allowed(&self, app_id: &str) -> bool {
        self.clipboard_apps
            .iter()
            .any(|pattern| glob::matches(pattern, app_id))
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
    use context_core::{ContentType, Sensitivity, WindowIdent};

    use super::*;

    fn policy() -> Policy {
        Policy::from_section(&VisualContext {
            excluded_apps: vec!["*bitwarden*".into(), "org.keepassxc.*".into()],
            clipboard_apps: vec!["code-oss".into()],
            ..VisualContext::default()
        })
        .unwrap()
    }

    fn window(app_id: &str, title: &str) -> WindowRecord {
        WindowRecord {
            ident: WindowIdent("w1".into()),
            app_id: app_id.into(),
            title: title.into(),
            ..WindowRecord::default()
        }
    }

    #[test]
    fn excluded_and_private_windows_vanish() {
        let policy = policy();
        assert_eq!(
            policy.judge(&window("com.bitwarden.desktop", "Vault")),
            Verdict::Drop(Reason::ExcludedApp)
        );
        assert_eq!(
            policy.judge(&window("firefox", "Mozilla Firefox Private Browsing")),
            Verdict::Drop(Reason::PrivateBrowsing)
        );
        assert_eq!(
            policy.judge(&window("chromium", "New Tab - Chromium (Incognito)")),
            Verdict::Drop(Reason::PrivateBrowsing)
        );
    }

    #[test]
    fn sensitive_and_moving_windows_keep_metadata_only() {
        let policy = policy();
        let mut typing_password = window("firefox", "Sign in");
        typing_password.sensitivity = Sensitivity {
            password_input: true,
            ..Sensitivity::default()
        };
        assert_eq!(
            policy.judge(&typing_password),
            Verdict::MetadataOnly(Reason::PasswordInput)
        );
        let mut video = window("mpv", "Lecture 3");
        video.content_type = ContentType::Video;
        assert_eq!(
            policy.judge(&video),
            Verdict::MetadataOnly(Reason::MotionContent)
        );
        assert!(!policy.judge(&video).allows_text());
        assert!(policy.judge(&video).allows_metadata());
    }

    #[test]
    fn plain_windows_are_observed_and_clipboard_is_per_app() {
        let policy = policy();
        assert_eq!(
            policy.judge(&window("code-oss", "main.rs")),
            Verdict::Observe
        );
        assert!(policy.clipboard_allowed("code-oss"));
        assert!(!policy.clipboard_allowed("firefox"));
    }

    #[test]
    fn a_bad_pattern_is_an_error_and_disabled_drops_everything() {
        let bad = VisualContext {
            private_titles: vec!["(".into()],
            ..VisualContext::default()
        };
        assert!(matches!(
            Policy::from_section(&bad),
            Err(PolicyError::Pattern { .. })
        ));
        let off = Policy::from_section(&VisualContext {
            enabled: false,
            ..VisualContext::default()
        })
        .unwrap();
        assert_eq!(
            off.judge(&window("code-oss", "x")),
            Verdict::Drop(Reason::Disabled)
        );
    }
}
