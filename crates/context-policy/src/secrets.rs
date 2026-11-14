//! Things that must never become a memory even when they are on screen.

use std::sync::LazyLock;

use regex::Regex;

use crate::util::luhn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    AwsAccessKey,
    PrivateKey,
    Jwt,
    ApiToken,
    PaymentCard,
    Iban,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    pub kind: SecretKind,
    pub start: usize,
    pub end: usize,
}

struct Detector {
    kind: SecretKind,
    pattern: Regex,
    verify: fn(&str) -> bool,
}

const fn always(_: &str) -> bool {
    true
}

fn luhn_digits(text: &str) -> bool {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    (13..=19).contains(&digits.len()) && luhn::is_valid(&digits)
}

fn iban(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    luhn::iban_is_valid(&compact)
}

static DETECTORS: LazyLock<Vec<Detector>> = LazyLock::new(|| {
    let detector = |kind, pattern: &str, verify| Detector {
        kind,
        pattern: Regex::new(pattern)
            .unwrap_or_else(|_| Regex::new("$^").unwrap_or_else(|_| unreachable!())),
        verify,
    };
    vec![
        detector(
            SecretKind::AwsAccessKey,
            r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
            always,
        ),
        detector(
            SecretKind::PrivateKey,
            r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----",
            always,
        ),
        detector(
            SecretKind::Jwt,
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
            always,
        ),
        detector(
            SecretKind::ApiToken,
            r"(?i)\b(?:api[_-]?key|secret|token|password|passwd|bearer)\b\s*[:=]\s*['\x22]?([A-Za-z0-9_\-./+=]{16,})",
            always,
        ),
        detector(
            SecretKind::ApiToken,
            r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b",
            always,
        ),
        detector(SecretKind::ApiToken, r"\bsk-[A-Za-z0-9]{20,}\b", always),
        detector(
            SecretKind::PaymentCard,
            r"\b(?:\d[ -]?){13,19}\b",
            luhn_digits,
        ),
        detector(
            SecretKind::Iban,
            r"\b[A-Z]{2}\d{2}(?:[ ]?[A-Z0-9]{4}){2,7}(?:[ ]?[A-Z0-9]{1,4})?\b",
            iban,
        ),
    ]
});

/// Every secret in `text`, in order of appearance.
pub fn find_secrets(text: &str) -> Vec<SecretMatch> {
    let mut found: Vec<SecretMatch> = DETECTORS
        .iter()
        .flat_map(|detector| {
            detector.pattern.find_iter(text).filter_map(|hit| {
                (detector.verify)(hit.as_str()).then_some(SecretMatch {
                    kind: detector.kind,
                    start: hit.start(),
                    end: hit.end(),
                })
            })
        })
        .collect();
    found.sort_by_key(|hit| (hit.start, hit.end));
    found
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

    fn kinds(text: &str) -> Vec<SecretKind> {
        find_secrets(text).into_iter().map(|hit| hit.kind).collect()
    }

    #[test]
    fn keys_tokens_and_cards_are_found() {
        assert_eq!(
            kinds("export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"),
            vec![SecretKind::AwsAccessKey]
        );
        assert_eq!(
            kinds("-----BEGIN RSA PRIVATE KEY-----"),
            vec![SecretKind::PrivateKey]
        );
        assert_eq!(
            kinds(
                "Authorization: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
            ),
            vec![SecretKind::Jwt]
        );
        assert_eq!(
            kinds("api_key = 'a1b2c3d4e5f6g7h8i9j0k1l2'"),
            vec![SecretKind::ApiToken]
        );
        assert_eq!(
            kinds("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"),
            vec![SecretKind::ApiToken]
        );
        assert_eq!(
            kinds("card 4111 1111 1111 1111 exp 12/29"),
            vec![SecretKind::PaymentCard]
        );
        assert_eq!(
            kinds("IBAN GB82 WEST 1234 5698 7654 32"),
            vec![SecretKind::Iban]
        );
    }

    #[test]
    fn ordinary_numbers_and_words_pass() {
        assert!(find_secrets("order 4111111111111112 shipped").is_empty());
        assert!(find_secrets("the token of appreciation was small").is_empty());
        assert!(find_secrets("commit 3f9a8c1 fixes #1234, see line 42").is_empty());
        assert!(find_secrets("phone +1 555 0100 2000").is_empty());
    }

    #[test]
    fn matches_come_back_in_text_order() {
        let text = "AKIAIOSFODNN7EXAMPLE then 4111111111111111";
        let found = find_secrets(text);
        assert_eq!(found.len(), 2);
        assert!(found[0].start < found[1].start);
        assert_eq!(&text[found[1].start..found[1].end], "4111111111111111");
    }
}
