//! Stage one of activity classification: what the app and the address say.

use context_core::ActivityLabel;

const APP_CONFIDENCE: f32 = 0.9;
const DOMAIN_CONFIDENCE: f32 = 0.75;
const BROWSER_DEFAULT_CONFIDENCE: f32 = 0.5;
const UNKNOWN_CONFIDENCE: f32 = 0.3;

const APPS: &[(&[&str], ActivityLabel)] = &[
    (
        &[
            "code",
            "vscode",
            "vscodium",
            "zed",
            "jetbrains",
            "idea",
            "neovim",
            "nvim",
            "emacs",
            "helix",
            "lapce",
        ],
        ActivityLabel::Coding,
    ),
    (
        &[
            "kitty",
            "alacritty",
            "foot",
            "konsole",
            "terminal",
            "wezterm",
            "ghostty",
            "tilix",
        ],
        ActivityLabel::TerminalOps,
    ),
    (
        &["thunderbird", "evolution", "geary", "kmail", "mailspring"],
        ActivityLabel::Email,
    ),
    (
        &[
            "slack",
            "discord",
            "telegram",
            "signal",
            "element",
            "whatsapp",
            "crownos-messages",
        ],
        ActivityLabel::CommunicationChat,
    ),
    (
        &["zoom", "teams", "meet", "jitsi", "webex"],
        ActivityLabel::Meeting,
    ),
    (&["calc", "gnumeric"], ActivityLabel::Spreadsheet),
    (
        &[
            "writer", "obsidian", "typora", "logseq", "joplin", "crownpad", "notes",
        ],
        ActivityLabel::Writing,
    ),
    (
        &[
            "gimp",
            "inkscape",
            "figma",
            "blender",
            "krita",
            "penpot",
            "darktable",
        ],
        ActivityLabel::Design,
    ),
    (
        &["nautilus", "dolphin", "thunar", "nemo", "crownos-files"],
        ActivityLabel::FileManagement,
    ),
    (
        &["settings", "control", "gnome-tweaks", "crownos-settings"],
        ActivityLabel::AdminSettings,
    ),
    (
        &["steam", "lutris", "heroic", "retroarch"],
        ActivityLabel::Gaming,
    ),
    (
        &["mpv", "vlc", "celluloid", "spotify", "rhythmbox"],
        ActivityLabel::MediaConsumption,
    ),
    (
        &[
            "okular",
            "evince",
            "zathura",
            "papers",
            "calibre",
            "crownos-preview",
        ],
        ActivityLabel::ReadingDocs,
    ),
];

const BROWSERS: &[&str] = &[
    "firefox",
    "chromium",
    "chrome",
    "brave",
    "vivaldi",
    "zen",
    "epiphany",
    "librewolf",
    "edge",
];

const DOMAINS: &[(&[&str], ActivityLabel)] = &[
    (
        &[
            "docs.rs",
            "readthedocs",
            "developer.mozilla.org",
            "doc.rust-lang.org",
            "wayland.app",
            "learn.microsoft.com",
            "wiki.archlinux.org",
            "man7.org",
        ],
        ActivityLabel::ReadingDocs,
    ),
    (
        &[
            "youtube.com",
            "netflix.com",
            "twitch.tv",
            "vimeo.com",
            "spotify.com",
            "primevideo.com",
        ],
        ActivityLabel::MediaConsumption,
    ),
    (
        &[
            "mail.google.com",
            "outlook.live.com",
            "outlook.office.com",
            "mail.proton.me",
            "fastmail.com",
        ],
        ActivityLabel::Email,
    ),
    (
        &[
            "amazon.",
            "ebay.",
            "aliexpress.",
            "etsy.com",
            "flipkart.com",
        ],
        ActivityLabel::Shopping,
    ),
    (
        &[
            "paypal.com",
            "stripe.com",
            "wise.com",
            "bank",
            "revolut.com",
        ],
        ActivityLabel::Finance,
    ),
    (
        &[
            "coursera.org",
            "udemy.com",
            "khanacademy.org",
            "edx.org",
            "leetcode.com",
        ],
        ActivityLabel::Learning,
    ),
    (
        &[
            "slack.com",
            "discord.com",
            "web.whatsapp.com",
            "web.telegram.org",
            "teams.microsoft.com",
        ],
        ActivityLabel::CommunicationChat,
    ),
    (
        &["meet.google.com", "zoom.us", "meet.jit.si"],
        ActivityLabel::Meeting,
    ),
    (
        &["figma.com", "penpot.app", "canva.com"],
        ActivityLabel::Design,
    ),
    (
        &["docs.google.com/document", "notion.so", "hackmd.io"],
        ActivityLabel::Writing,
    ),
    (
        &["docs.google.com/spreadsheets", "airtable.com"],
        ActivityLabel::Spreadsheet,
    ),
];

/// The label and its confidence, from the app id, the document URL and the title.
pub fn classify(app_id: &str, url: Option<&str>, title: &str) -> (ActivityLabel, f32) {
    let app = app_id.to_ascii_lowercase();
    if BROWSERS.iter().any(|browser| app.contains(browser)) {
        return classify_page(url, title);
    }
    for (names, label) in APPS {
        if names.iter().any(|name| app.contains(name)) {
            return (*label, APP_CONFIDENCE);
        }
    }
    (ActivityLabel::Other, UNKNOWN_CONFIDENCE)
}

fn classify_page(url: Option<&str>, title: &str) -> (ActivityLabel, f32) {
    let lower_title = title.to_ascii_lowercase();
    if let Some(url) = url.map(str::to_ascii_lowercase) {
        if url.contains("github.com") || url.contains("gitlab.com") || url.contains("codeberg.org")
        {
            return if url.contains("/pull/")
                || url.contains("/merge_requests/")
                || url.contains("/compare")
            {
                (ActivityLabel::CodeReview, DOMAIN_CONFIDENCE)
            } else {
                (ActivityLabel::Coding, DOMAIN_CONFIDENCE)
            };
        }
        for (domains, label) in DOMAINS {
            if domains.iter().any(|domain| url.contains(domain)) {
                return (*label, DOMAIN_CONFIDENCE);
            }
        }
    }
    if lower_title.contains("pull request") || lower_title.contains("merge request") {
        return (ActivityLabel::CodeReview, DOMAIN_CONFIDENCE);
    }
    (ActivityLabel::ResearchBrowsing, BROWSER_DEFAULT_CONFIDENCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_name_their_activity() {
        assert_eq!(classify("code-oss", None, "x").0, ActivityLabel::Coding);
        assert_eq!(classify("kitty", None, "x").0, ActivityLabel::TerminalOps);
        assert_eq!(
            classify("org.mozilla.Thunderbird", None, "Inbox").0,
            ActivityLabel::Email
        );
        assert_eq!(
            classify("com.slack.Slack", None, "x").0,
            ActivityLabel::CommunicationChat
        );
        assert_eq!(
            classify("unknown.app", None, "x"),
            (ActivityLabel::Other, UNKNOWN_CONFIDENCE)
        );
    }

    #[test]
    fn browsers_are_judged_by_the_page() {
        assert_eq!(
            classify("firefox", Some("https://docs.rs/smithay"), "").0,
            ActivityLabel::ReadingDocs
        );
        assert_eq!(
            classify("firefox", Some("https://github.com/a/b/pull/4"), "").0,
            ActivityLabel::CodeReview
        );
        assert_eq!(
            classify("chromium", Some("https://github.com/a/b/blob/x.rs"), "").0,
            ActivityLabel::Coding
        );
        assert_eq!(
            classify("firefox", Some("https://www.youtube.com/watch?v=1"), "").0,
            ActivityLabel::MediaConsumption
        );
        assert_eq!(
            classify("firefox", None, "Fix flicker · Pull Request #9").0,
            ActivityLabel::CodeReview
        );
        assert_eq!(
            classify("firefox", None, "Some page").0,
            ActivityLabel::ResearchBrowsing
        );
    }
}
