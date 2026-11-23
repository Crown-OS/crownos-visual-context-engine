//! The Flatpak app a process belongs to, from the `.flatpak-info` file that Flatpak puts at the
//! root of the sandbox.

const APPLICATION_GROUP: &str = "[Application]";
const NAME_KEY: &str = "name=";

/// The app id of the Flatpak sandbox `pid` runs in, or `None` for a process outside one.
pub(crate) fn flatpak_app_id(pid: i32) -> Option<String> {
    let info = std::fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")).ok()?;
    app_id_in(&info)
}

fn app_id_in(info: &str) -> Option<String> {
    let mut in_application = false;
    for line in info.lines().map(str::trim) {
        if line.starts_with('[') {
            in_application = line == APPLICATION_GROUP;
        } else if in_application && let Some(name) = line.strip_prefix(NAME_KEY) {
            let name = name.trim();
            return (!name.is_empty()).then(|| name.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_id_comes_from_the_application_group() {
        let info = "[Instance]\nname=not-this\n\n[Application]\nname=org.example.Notes\n";
        assert_eq!(app_id_in(info).as_deref(), Some("org.example.Notes"));
        assert_eq!(app_id_in("[Application]\nruntime=x\n"), None);
        assert_eq!(flatpak_app_id(std::process::id().cast_signed()), None);
    }
}
