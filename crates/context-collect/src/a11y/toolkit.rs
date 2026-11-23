const NATIVE_DEPTH: usize = 12;
const WEB_DEPTH: usize = 24;

/// The toolkit behind an application on the a11y bus, from its `ToolkitName` and `Version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Toolkit {
    /// GTK 4 publishes the whole tree through `Cache.GetItems` and has no `Collection`.
    Gtk4,
    /// Qt 6 implements `Collection.GetMatches`.
    Qt,
    /// GTK 3, Firefox and the other ATK toolkits: the ATK bridge implements `Collection`.
    AtkBridge,
    /// Chromium and Electron raise their accessibility mode by the calls they receive.
    Chromium,
    Unknown,
}

/// How the structure of a window is fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStrategy {
    CollectionMatches,
    CacheItems,
    BoundedWalk,
}

impl Toolkit {
    /// Chromium and Electron are matched first: they sit behind the ATK bridge too, but must not
    /// get `GetMatches`.
    pub fn identify(name: &str, version: &str) -> Self {
        let name = name.trim();
        if name.eq_ignore_ascii_case("chromium") || name.eq_ignore_ascii_case("electron") {
            Self::Chromium
        } else if name.eq_ignore_ascii_case("gtk") {
            match version.trim().split('.').next() {
                Some("4") => Self::Gtk4,
                _ => Self::AtkBridge,
            }
        } else if name.eq_ignore_ascii_case("qt") {
            Self::Qt
        } else if name.eq_ignore_ascii_case("gecko") {
            Self::AtkBridge
        } else {
            Self::Unknown
        }
    }

    /// How deep below the frame a fetch goes. Web content nests far deeper than native widgets.
    pub const fn walk_depth(self) -> usize {
        match self {
            Self::Chromium | Self::Unknown => WEB_DEPTH,
            Self::Gtk4 | Self::Qt | Self::AtkBridge => NATIVE_DEPTH,
        }
    }

    pub const fn strategy(self) -> FetchStrategy {
        match self {
            Self::Qt | Self::AtkBridge => FetchStrategy::CollectionMatches,
            Self::Gtk4 => FetchStrategy::CacheItems,
            Self::Chromium | Self::Unknown => FetchStrategy::BoundedWalk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_seen_on_the_bus_map_to_toolkits() {
        assert_eq!(Toolkit::identify("gtk", "3.24.52"), Toolkit::AtkBridge);
        assert_eq!(Toolkit::identify("GTK", "4.24.1"), Toolkit::Gtk4);
        assert_eq!(Toolkit::identify("Qt", "6.12.0"), Toolkit::Qt);
        assert_eq!(Toolkit::identify("Chromium", "142.0"), Toolkit::Chromium);
        assert_eq!(Toolkit::identify("Gecko", "146.0"), Toolkit::AtkBridge);
        assert_eq!(Toolkit::identify("VCL", "25.8"), Toolkit::Unknown);
    }

    #[test]
    fn each_toolkit_gets_its_cheapest_safe_strategy() {
        assert_eq!(Toolkit::Qt.strategy(), FetchStrategy::CollectionMatches);
        assert_eq!(
            Toolkit::AtkBridge.strategy(),
            FetchStrategy::CollectionMatches
        );
        assert_eq!(Toolkit::Gtk4.strategy(), FetchStrategy::CacheItems);
        assert_eq!(Toolkit::Chromium.strategy(), FetchStrategy::BoundedWalk);
        assert_eq!(Toolkit::Unknown.strategy(), FetchStrategy::BoundedWalk);
    }
}
