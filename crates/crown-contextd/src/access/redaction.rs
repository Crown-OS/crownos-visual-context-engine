//! What a caller with fewer scopes sees of the current context.

use context_protocol::CurrentContext;
use crownos_config::schema::{Scope, ScopeSet};

/// The context as `scopes` allow: nothing without metadata, and without
/// text no titles, documents, projects or episode titles.
pub(crate) fn redact_context(context: &CurrentContext, scopes: ScopeSet) -> Option<CurrentContext> {
    if !scopes.contains(Scope::Metadata) {
        return None;
    }
    let mut context = context.clone();
    if scopes.contains(Scope::Text) {
        return Some(context);
    }
    if let Some(focus) = &mut context.focus {
        focus.title.clear();
        focus.doc_key.clear();
        focus.project = None;
    }
    if let Some(episode) = &mut context.episode {
        episode.title.clear();
    }
    context.recent_documents.clear();
    Some(context)
}

#[cfg(test)]
mod tests {
    use context_core::{AppRef, Timestamp};
    use context_protocol::FocusView;

    use super::*;

    fn context() -> CurrentContext {
        CurrentContext {
            t: Timestamp(0),
            focus: Some(FocusView {
                app: AppRef::default(),
                title: "Salary review.ods".to_owned(),
                doc_key: "file:/home/u/salary.ods".to_owned(),
                project: Some("hr".to_owned()),
            }),
            activity: None,
            episode: None,
            recent_documents: vec!["file:/home/u/salary.ods".to_owned()],
            open_documents: 1,
            observing: true,
        }
    }

    #[test]
    fn metadata_callers_get_no_titles_or_documents() {
        let metadata = ScopeSet::EMPTY.with(Scope::Metadata);
        let redacted = redact_context(&context(), metadata);
        let focus = redacted.as_ref().and_then(|context| context.focus.as_ref());
        assert_eq!(focus.map(|focus| focus.title.as_str()), Some(""));
        assert_eq!(focus.map(|focus| focus.doc_key.as_str()), Some(""));
        assert!(redacted.is_some_and(|context| context.recent_documents.is_empty()));
    }

    #[test]
    fn text_callers_get_everything_and_others_nothing() {
        let text = ScopeSet::EMPTY.with(Scope::Metadata).with(Scope::Text);
        assert_eq!(redact_context(&context(), text), Some(context()));
        assert_eq!(
            redact_context(&context(), ScopeSet::EMPTY.with(Scope::Pause)),
            None
        );
    }
}
