/// Whether a run of digits passes the Luhn check, as every payment card number does.
pub(crate) fn is_valid(digits: &str) -> bool {
    let mut sum = 0;
    let mut double = false;
    for digit in digits.bytes().rev() {
        if !digit.is_ascii_digit() {
            return false;
        }
        let mut value = u32::from(digit - b'0');
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum % 10 == 0 && !digits.is_empty()
}

/// The mod-97 check of an IBAN: letters become two digits, the first four characters move last.
pub(crate) fn iban_is_valid(iban: &str) -> bool {
    let (head, tail) = match iban.len() {
        15..=34 => iban.split_at(4),
        _ => return false,
    };
    let rearranged = format!("{tail}{head}");
    let mut remainder = 0u32;
    for character in rearranged.chars() {
        let value = match character {
            '0'..='9' => u32::from(character) - u32::from('0'),
            'A'..='Z' => u32::from(character) - u32::from('A') + 10,
            _ => return false,
        };
        let step = if value >= 10 { 100 } else { 10 };
        remainder = (remainder * step + value) % 97;
    }
    remainder == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luhn_accepts_test_cards_only() {
        assert!(is_valid("4111111111111111"));
        assert!(is_valid("5500000000000004"));
        assert!(!is_valid("4111111111111112"));
        assert!(!is_valid(""));
    }

    #[test]
    fn iban_check_digits_are_verified() {
        assert!(iban_is_valid("GB82WEST12345698765432"));
        assert!(iban_is_valid("DE89370400440532013000"));
        assert!(!iban_is_valid("DE89370400440532013001"));
        assert!(!iban_is_valid("SHORT"));
    }
}
