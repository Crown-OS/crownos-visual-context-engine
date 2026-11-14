//! The systemd desktop-environment cgroup conventions, as the process monitor reads them.

/// What a cgroup v2 path says about the app that owns a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupIdentity {
    /// `app-[<launcher>-]<app id>-<random>.scope` or `app-[<launcher>-]<app id>@<random>.service`.
    App(String),
    Flatpak(String),
    Snap(String),
    /// `<terminal>-<pid>-<n>.scope`.
    Terminal(String),
    Service(String),
    Session,
    Unknown,
}

/// The cgroup path from a `/proc/<pid>/cgroup` text (the v2 `0::` line).
pub fn unified_path(text: &str) -> &str {
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .unwrap_or("/")
        .trim()
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `app-gnome-org.mozilla.firefox-1234.scope` → `org.mozilla.firefox`: drop the launcher and
/// the random suffix; systemd escapes `-` in names as `\x2d`.
fn app_unit(body: &str) -> Option<CgroupIdentity> {
    let (name, _random) = body
        .strip_suffix(".scope")
        .and_then(|rest| rest.rsplit_once('-'))
        .or_else(|| {
            body.strip_suffix(".service")
                .and_then(|rest| rest.rsplit_once('@'))
        })?;
    let name = unescape(name);
    if let Some(flatpak) = name.strip_prefix("flatpak-") {
        return Some(CgroupIdentity::Flatpak(flatpak.to_owned()));
    }
    let mut parts = name.splitn(2, '-');
    let first = parts.next()?;
    let rest = parts.next();
    let app_id = match rest {
        Some(rest) if is_launcher(first) => rest,
        _ => &name,
    };
    Some(CgroupIdentity::App(app_id.to_owned()))
}

fn is_launcher(part: &str) -> bool {
    matches!(
        part,
        "gnome" | "kde" | "crownos" | "crownlauncher" | "dbus" | "systemd"
    )
}

fn unescape(text: &str) -> String {
    text.replace("\\x2d", "-").replace("\\x2f", "/")
}

pub fn parse_cgroup(path: &str) -> CgroupIdentity {
    let leaf = leaf(path);
    if let Some(body) = leaf
        .strip_prefix("snap.")
        .and_then(|body| body.strip_suffix(".scope"))
    {
        return CgroupIdentity::Snap(body.split('.').next().unwrap_or(body).to_owned());
    }
    if let Some(identity) = leaf.strip_prefix("app-").and_then(app_unit) {
        return identity;
    }
    if leaf.starts_with("session-") && leaf.ends_with(".scope") {
        return CgroupIdentity::Session;
    }
    if let Some(scope) = leaf.strip_suffix(".scope") {
        let mut parts = scope.rsplitn(3, '-');
        let (Some(_n), Some(pid), Some(terminal)) = (parts.next(), parts.next(), parts.next())
        else {
            return CgroupIdentity::Unknown;
        };
        if pid.bytes().all(|byte| byte.is_ascii_digit()) {
            return CgroupIdentity::Terminal(terminal.to_owned());
        }
        return CgroupIdentity::Unknown;
    }
    if let Some(service) = leaf.strip_suffix(".service") {
        return CgroupIdentity::Service(service.to_owned());
    }
    CgroupIdentity::Unknown
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
    fn desktop_scopes_name_the_app() {
        assert_eq!(
            parse_cgroup(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-org.mozilla.firefox-4202.scope"
            ),
            CgroupIdentity::App("org.mozilla.firefox".into())
        );
        assert_eq!(
            parse_cgroup(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/app-code\\x2doss-9.scope"
            ),
            CgroupIdentity::App("code-oss".into())
        );
        assert_eq!(
            parse_cgroup("/u/app.slice/app-flatpak-com.slack.Slack-12.scope"),
            CgroupIdentity::Flatpak("com.slack.Slack".into())
        );
        assert_eq!(
            parse_cgroup("/u/app.slice/app-org.kde.konsole@abc.service"),
            CgroupIdentity::App("org.kde.konsole".into())
        );
    }

    #[test]
    fn terminals_snaps_sessions_and_services_are_told_apart() {
        assert_eq!(
            parse_cgroup("/u/app.slice/kitty-4303-0.scope"),
            CgroupIdentity::Terminal("kitty".into())
        );
        assert_eq!(
            parse_cgroup("/u/snap.firefox.firefox-1.scope"),
            CgroupIdentity::Snap("firefox".into())
        );
        assert_eq!(
            parse_cgroup("/user.slice/user-1000.slice/session-2.scope"),
            CgroupIdentity::Session
        );
        assert_eq!(
            parse_cgroup("/u/pipewire.service"),
            CgroupIdentity::Service("pipewire".into())
        );
        assert_eq!(parse_cgroup("/"), CgroupIdentity::Unknown);
    }

    #[test]
    fn the_unified_line_is_the_one_read() {
        assert_eq!(
            unified_path("1:name=systemd:/x\n0::/user.slice/a.scope\n"),
            "/user.slice/a.scope"
        );
        assert_eq!(unified_path(""), "/");
    }
}
