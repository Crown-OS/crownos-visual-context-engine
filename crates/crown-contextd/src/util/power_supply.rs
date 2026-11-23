use std::path::Path;

const POWER_SUPPLIES: &str = "/sys/class/power_supply";

/// Whether the machine runs on a battery now: it has a battery and no mains supply is online.
/// A machine without power supply information counts as plugged in.
pub(crate) fn on_battery() -> bool {
    let Ok(supplies) = std::fs::read_dir(POWER_SUPPLIES) else {
        return false;
    };
    let mut has_battery = false;
    for supply in supplies.flatten() {
        let path = supply.path();
        match read_trimmed(&path.join("type")).as_deref() {
            Some("Mains" | "USB") if read_trimmed(&path.join("online")).as_deref() == Some("1") => {
                return false;
            }
            Some("Battery") => has_battery = true,
            _ => {}
        }
    }
    has_battery
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}
