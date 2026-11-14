use std::collections::BTreeMap;

crownos_config::section! {
    /// What the visual context engine may observe, and for how long it keeps it.
    pub struct VisualContext in "visual_context", keys VisualContextKeys {
        /// Whether the engine observes at all.
        pub enabled as Enabled: bool = true,
        /// app_id globs never observed: no metadata, no text, no captures.
        pub excluded_apps as ExcludedApps: Vec<String> = Vec::new(),
        /// Title patterns (regular expressions) that mark a window as private browsing.
        pub private_titles as PrivateTitles: Vec<String> = default_private_titles(),
        /// app_id globs whose clipboard content may be read; metadata is kept for all.
        pub clipboard_apps as ClipboardApps: Vec<String> = Vec::new(),
        /// Days raw text lines and frames are kept.
        pub retention_raw_days as RetentionRawDays: u32 = 30,
        /// Days thumbnails are kept.
        pub retention_thumbnail_days as RetentionThumbnailDays: u32 = 7,
        /// Days segments and episodes are kept.
        pub retention_segment_days as RetentionSegmentDays: u32 = 365,
        /// Whether thumbnails are stored at all.
        pub thumbnails as Thumbnails: bool = false,
        /// The chord that pauses observation.
        pub pause_hotkey as PauseHotkey: String = "Super+Shift+.".to_owned(),
        /// Scopes granted per caller app key, e.g. `"crownos-computer-use": ["context.current"]`.
        pub grants as Grants: BTreeMap<String, Vec<String>> = BTreeMap::new(),
    }
}

/// Private windows name themselves in the title in every browser, in the user's language.
fn default_private_titles() -> Vec<String> {
    [
        r"(?i)\bprivate browsing\b",
        r"(?i)\bincognito\b",
        r"(?i)\bInPrivate\b",
        r"(?i)\bnavigation privée\b",
        r"(?i)\bprivates fenster\b",
        r"(?i)\bnavegación privada\b",
        r"(?i)\bnavigazione anonima\b",
        r"(?i)\bprivé-venster\b",
        r"(?i)\bjanela privada\b",
        r"(?i)\bprywatne\b",
        r"(?i)\bприватн",
        r"プライベートブラウジング",
        r"隐身|無痕|隱私",
    ]
    .map(str::to_owned)
    .to_vec()
}
