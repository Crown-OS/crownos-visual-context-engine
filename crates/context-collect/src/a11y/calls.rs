//! Every AT-SPI call the collector can make. Chromium raises its accessibility mode, and with it
//! its memory and CPU cost, by the calls it receives: text boundaries and extents add inline text
//! boxes, `GetAttributes` and `GetRelationSet` add extended properties. Those calls have no
//! variant here, so the collector cannot make them.

use super::toolkit::{FetchStrategy, Toolkit};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtspiCall {
    Role,
    Name,
    State,
    Children,
    /// `Text.GetText(0, -1)`: the whole text, never a boundary or a range.
    WholeText,
    /// `Document.GetAttributeValue("DocURL")`.
    DocumentUrl,
    /// The `ToolkitName` and `Version` properties of the application root.
    Toolkit,
    CollectionMatches,
    CacheItems,
}

impl AtspiCall {
    /// The interface and member that go on the wire.
    pub const fn wire(self) -> (&'static str, &'static str) {
        match self {
            Self::Role => (ACCESSIBLE, "GetRole"),
            Self::Name | Self::Toolkit => (PROPERTIES, "Get"),
            Self::State => (ACCESSIBLE, "GetState"),
            Self::Children => (ACCESSIBLE, "GetChildren"),
            Self::WholeText => ("org.a11y.atspi.Text", "GetText"),
            Self::DocumentUrl => ("org.a11y.atspi.Document", "GetAttributeValue"),
            Self::CollectionMatches => ("org.a11y.atspi.Collection", "GetMatches"),
            Self::CacheItems => ("org.a11y.atspi.Cache", "GetItems"),
        }
    }

    /// The plain reads go to every toolkit; a bulk call only where the toolkit's strategy uses it.
    pub fn permitted(self, toolkit: Toolkit) -> bool {
        match self {
            Self::Role
            | Self::Name
            | Self::State
            | Self::Children
            | Self::WholeText
            | Self::DocumentUrl
            | Self::Toolkit => true,
            Self::CollectionMatches => toolkit.strategy() == FetchStrategy::CollectionMatches,
            Self::CacheItems => toolkit.strategy() == FetchStrategy::CacheItems,
        }
    }

    /// Every call, so tests can check the whole list.
    pub const ALL: [Self; 9] = [
        Self::Role,
        Self::Name,
        Self::State,
        Self::Children,
        Self::WholeText,
        Self::DocumentUrl,
        Self::Toolkit,
        Self::CollectionMatches,
        Self::CacheItems,
    ];

    /// The calls that raise Chromium's accessibility mode. None of them is an [`AtspiCall`].
    pub const MODE_RAISING: [(&'static str, &'static str); 12] = [
        ("org.a11y.atspi.Text", "GetTextAtOffset"),
        ("org.a11y.atspi.Text", "GetTextBeforeOffset"),
        ("org.a11y.atspi.Text", "GetTextAfterOffset"),
        ("org.a11y.atspi.Text", "GetStringAtOffset"),
        ("org.a11y.atspi.Text", "GetCharacterExtents"),
        ("org.a11y.atspi.Text", "GetRangeExtents"),
        ("org.a11y.atspi.Text", "GetBoundedRanges"),
        ("org.a11y.atspi.Text", "GetAttributes"),
        ("org.a11y.atspi.Component", "GetExtents"),
        ("org.a11y.atspi.Accessible", "GetAttributes"),
        ("org.a11y.atspi.Accessible", "GetRelationSet"),
        ("org.a11y.atspi.Document", "GetAttributes"),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_TOOLKIT: [Toolkit; 5] = [
        Toolkit::Gtk4,
        Toolkit::Qt,
        Toolkit::AtkBridge,
        Toolkit::Chromium,
        Toolkit::Unknown,
    ];

    /// Fails to compile when a call is added without a place in [`AtspiCall::ALL`].
    const fn position(call: AtspiCall) -> usize {
        match call {
            AtspiCall::Role => 0,
            AtspiCall::Name => 1,
            AtspiCall::State => 2,
            AtspiCall::Children => 3,
            AtspiCall::WholeText => 4,
            AtspiCall::DocumentUrl => 5,
            AtspiCall::Toolkit => 6,
            AtspiCall::CollectionMatches => 7,
            AtspiCall::CacheItems => 8,
        }
    }

    #[test]
    fn the_list_holds_every_call_once() {
        for (index, call) in AtspiCall::ALL.into_iter().enumerate() {
            assert_eq!(position(call), index);
        }
    }

    #[test]
    fn no_call_raises_the_chromium_mode() {
        for call in AtspiCall::ALL {
            assert!(!AtspiCall::MODE_RAISING.contains(&call.wire()), "{call:?}");
        }
    }

    #[test]
    fn chromium_and_unknown_toolkits_get_only_plain_reads() {
        let plain =
            |call: AtspiCall| !matches!(call, AtspiCall::CollectionMatches | AtspiCall::CacheItems);
        for toolkit in [Toolkit::Chromium, Toolkit::Unknown] {
            for call in AtspiCall::ALL {
                assert_eq!(call.permitted(toolkit), plain(call), "{toolkit:?} {call:?}");
            }
        }
    }

    #[test]
    fn bulk_calls_go_only_where_the_toolkit_implements_them() {
        for toolkit in EVERY_TOOLKIT {
            assert_eq!(
                AtspiCall::CollectionMatches.permitted(toolkit),
                matches!(toolkit, Toolkit::Qt | Toolkit::AtkBridge)
            );
            assert_eq!(
                AtspiCall::CacheItems.permitted(toolkit),
                toolkit == Toolkit::Gtk4
            );
        }
    }
}
