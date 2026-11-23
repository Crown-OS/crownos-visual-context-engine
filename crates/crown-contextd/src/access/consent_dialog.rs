//! The first-use question for an API caller. contextd runs the root-owned consent dialog
//! itself and reads only its exit status: no D-Bus name and no output stream is in the path,
//! so no other process can answer for the user.

use crownos_config::consent_dialog::{ConsentOption, ConsentQuestion};
use crownos_config::schema::AppKey;

const REMEMBER: usize = 0;
const SCOPE: usize = 1;
const TEXT_ITEM: usize = 1;

/// What the user said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccessAnswer {
    pub(crate) granted: bool,
    pub(crate) remember: bool,
    /// The user also allowed window titles and text.
    pub(crate) text: bool,
}

/// Asks whether the app at `key` may use the Context API.
pub(crate) async fn ask_api_access(key: &AppKey) -> Result<AccessAnswer, String> {
    let question = question(key);
    let status = tokio::process::Command::from(question.command())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| format!("cannot run the consent dialog: {error}"))?;
    let answer = question
        .answer(status.code())
        .map_err(|error| error.to_string())?;
    Ok(AccessAnswer {
        granted: answer.granted,
        remember: answer.checked(REMEMBER),
        text: answer.chosen(SCOPE) == Some(TEXT_ITEM),
    })
}

fn question(key: &AppKey) -> ConsentQuestion {
    let subtitle = format!("{key} asks what you work on.");
    let body = "With the window list it sees which apps you use. With titles it can \
                also search what you worked on, and see your timeline and episodes. \
                You can take this back any time in Settings.";
    ConsentQuestion {
        dangerous: false,
        title: format!("Let {} use your screen context?", key.display_name()),
        message: format!("{subtitle}\n\n{body}"),
        grant: "Allow".to_owned(),
        deny: "Don't allow".to_owned(),
        options: vec![
            ConsentOption::Checkbox {
                label: "Remember this choice".to_owned(),
                initially_on: false,
            },
            ConsentOption::List {
                label: "The app may see".to_owned(),
                items: vec![
                    "Window list".to_owned(),
                    "Window list, titles and search".to_owned(),
                ],
                initial: 0,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_asks_to_remember_and_the_scope() {
        let key = AppKey::HostExecutable {
            path: "/usr/bin/notes".into(),
        };
        let arguments = question(&key).arguments();
        let text = arguments.join(" ");
        assert!(text.contains("--title Let notes use your screen context?"));
        assert!(text.contains("--checkbox Remember this choice --initial false"));
        assert!(text.contains("--item Window list, titles and search --initial 0"));
    }
}
