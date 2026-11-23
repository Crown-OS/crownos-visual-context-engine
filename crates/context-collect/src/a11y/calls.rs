//! Every AT-SPI call the collector can make. Chromium raises its accessibility mode, and with it
//! its memory and CPU cost, by the calls it receives: text boundaries and extents add inline text
//! boxes, `GetAttributes` and `GetRelationSet` add extended properties. Those calls have no
//! variant here, so the collector cannot make them.

use super::toolkit::Toolkit;

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

    pub const fn permitted(self, toolkit: Toolkit) -> bool {
        match self {
            Self::Role
            | Self::Name
            | Self::State
            | Self::Children
            | Self::WholeText
            | Self::DocumentUrl
            | Self::Toolkit => true,
            Self::CollectionMatches => matches!(toolkit, Toolkit::Qt | Toolkit::AtkBridge),
            Self::CacheItems => matches!(toolkit, Toolkit::Gtk4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_CALL: [AtspiCall; 9] = [
        AtspiCall::Role,
        AtspiCall::Name,
        AtspiCall::State,
        AtspiCall::Children,
        AtspiCall::WholeText,
        AtspiCall::DocumentUrl,
        AtspiCall::Toolkit,
        AtspiCall::CollectionMatches,
        AtspiCall::CacheItems,
    ];

    const EVERY_TOOLKIT: [Toolkit; 5] = [
        Toolkit::Gtk4,
        Toolkit::Qt,
        Toolkit::AtkBridge,
        Toolkit::Chromium,
        Toolkit::Unknown,
    ];

    /// The calls that raise Chromium's accessibility mode.
    const MODE_RAISING: [(&str, &str); 12] = [
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

    #[test]
    fn no_call_raises_the_chromium_mode() {
        for call in EVERY_CALL {
            assert!(!MODE_RAISING.contains(&call.wire()), "{call:?}");
        }
    }

    #[test]
    fn chromium_and_unknown_toolkits_get_only_plain_reads() {
        let plain = |call: &AtspiCall| {
            !matches!(call, AtspiCall::CollectionMatches | AtspiCall::CacheItems)
        };
        for toolkit in [Toolkit::Chromium, Toolkit::Unknown] {
            for call in EVERY_CALL {
                assert_eq!(
                    call.permitted(toolkit),
                    plain(&call),
                    "{toolkit:?} {call:?}"
                );
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
