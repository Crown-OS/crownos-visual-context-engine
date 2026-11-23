//! The first-use question for an API caller, through the desktop portal's
//! Access dialog, the same one the compositor asks with.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crownos_config::schema::AppKey;
use zbus::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const BACKENDS: [&str; 2] = [
    "org.freedesktop.impl.portal.desktop.crownos",
    "org.freedesktop.impl.portal.desktop.gtk",
];
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.impl.portal.Access";
const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";
const REMEMBER: &str = "remember";
const SCOPE: &str = "scope";
const METADATA: &str = "metadata";
const TEXT: &str = "text";
const GRANTED: u32 = 0;

/// One `a(ssa(ss)s)` entry of the `choices` option: id, label, items, initial.
type Choice<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>, &'a str);

/// What the user said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PortalAnswer {
    pub(crate) granted: bool,
    pub(crate) remember: bool,
    /// The user also allowed window titles and text.
    pub(crate) text: bool,
}

/// Asks whether the app at `key` may use the Context API.
pub(crate) async fn ask_api_access(key: &AppKey) -> Result<PortalAnswer, String> {
    static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    let connection = Connection::session()
        .await
        .map_err(|error| error.to_string())?;
    let handle = ObjectPath::try_from(format!("/org/crownos/contextd/request/{ticket}"))
        .map_err(|error| error.to_string())?;
    let title = format!("Let {} use your screen context?", display_name(key));
    let subtitle = format!("{key} asks what you work on.");
    let body = "With the window list it sees which apps you use. With titles it can \
                also search what you worked on, and see your timeline and episodes. \
                You can take this back any time in Settings.";
    let choices: Vec<Choice<'_>> = vec![
        (REMEMBER, "Remember this choice", Vec::new(), "false"),
        (
            SCOPE,
            "The app may see",
            vec![
                (METADATA, "Window list"),
                (TEXT, "Window list, titles and search"),
            ],
            METADATA,
        ),
    ];
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("modal", true.into());
    options.insert("grant_label", "Allow".into());
    options.insert("deny_label", "Don't allow".into());
    options.insert("choices", Value::new(choices));
    let key_text = key.to_string();
    for backend in BACKENDS {
        let reply = connection
            .call_method(
                Some(backend),
                PATH,
                Some(INTERFACE),
                "AccessDialog",
                &(
                    &handle,
                    key_text.as_str(),
                    "",
                    title.as_str(),
                    subtitle.as_str(),
                    body,
                    &options,
                ),
            )
            .await;
        match reply {
            Ok(reply) => {
                let (response, results): (u32, HashMap<String, OwnedValue>) = reply
                    .body()
                    .deserialize()
                    .map_err(|error| error.to_string())?;
                let choices = results
                    .get("choices")
                    .and_then(|choices| <Vec<(String, String)>>::try_from(choices.clone()).ok())
                    .unwrap_or_default();
                let chosen = |id: &str| {
                    choices
                        .iter()
                        .find(|(choice, _)| choice == id)
                        .map(|(_, value)| value.as_str())
                };
                return Ok(PortalAnswer {
                    granted: response == GRANTED,
                    remember: chosen(REMEMBER) == Some("true"),
                    text: chosen(SCOPE) == Some(TEXT),
                });
            }
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == SERVICE_UNKNOWN => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("no portal backend offers an access dialog".to_owned())
}

fn display_name(key: &AppKey) -> String {
    let file_name = |path: &std::path::Path| {
        path.file_name().map_or_else(
            || key.to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    match key {
        AppKey::Sandboxed { app_id, .. } => app_id.clone(),
        AppKey::Spawned { component } => component.clone(),
        AppKey::HostExecutable { path } => file_name(path),
        AppKey::Interpreted { script, .. } => file_name(script),
    }
}
