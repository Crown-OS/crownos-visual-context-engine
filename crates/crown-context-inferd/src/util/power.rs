use std::path::Path;

const POWER_SUPPLIES: &str = "/sys/class/power_supply";

/// True when a battery is present and no mains or USB supply is online.
pub(crate) fn on_battery() -> bool {
    on_battery_in(Path::new(POWER_SUPPLIES))
}

fn on_battery_in(supplies: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(supplies) else {
        return false;
    };
    let read = |path: &Path, name: &str| {
        std::fs::read_to_string(path.join(name))
            .map(|value| value.trim().to_owned())
            .unwrap_or_default()
    };
    let (mut battery, mut external_power) = (false, false);
    for entry in entries.flatten() {
        let path = entry.path();
        match read(&path, "type").as_str() {
            "Battery" => battery = true,
            "Mains" | "USB" => external_power |= read(&path, "online") == "1",
            _ => {}
        }
    }
    battery && !external_power
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, kind: &str, online: &str) {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("type"), kind).unwrap();
        std::fs::write(directory.join("online"), online).unwrap();
    }

    #[test]
    fn a_laptop_unplugged_is_on_battery() {
        let root = std::env::temp_dir().join(format!("inferd-power-{}", std::process::id()));
        supply(&root, "BAT0", "Battery\n", "");
        supply(&root, "AC", "Mains\n", "0\n");
        assert!(on_battery_in(&root));
        supply(&root, "AC", "Mains\n", "1\n");
        assert!(!on_battery_in(&root));
        std::fs::remove_dir_all(&root).unwrap();
        assert!(!on_battery_in(&root));
    }
}
