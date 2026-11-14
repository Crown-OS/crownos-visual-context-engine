/// Whether `text` matches `pattern`, where `*` spans anything and `?` one character, both
/// case-insensitively, since app ids differ in case between toolkits.
pub(crate) fn matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    matches_at(&pattern, &text)
}

fn matches_at(pattern: &[char], text: &[char]) -> bool {
    match (pattern.split_first(), text.split_first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(('*', rest)), _) => {
            (0..=text.len()).any(|skip| matches_at(rest, text.get(skip..).unwrap_or_default()))
        }
        (Some(('?', rest)), Some((_, text_rest))) => matches_at(rest, text_rest),
        (Some((literal, rest)), Some((head, text_rest))) => {
            *literal == *head && matches_at(rest, text_rest)
        }
        (Some(_), None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_span_and_questions_take_one() {
        assert!(matches("org.mozilla.*", "org.mozilla.firefox"));
        assert!(matches("*bitwarden*", "com.bitwarden.desktop"));
        assert!(matches("kitt?", "Kitty"));
        assert!(!matches("kitt?", "kitten"));
        assert!(!matches("firefox", "org.mozilla.firefox"));
        assert!(matches("*", ""));
    }
}
