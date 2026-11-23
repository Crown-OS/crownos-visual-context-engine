/// Joins token pieces that may split a UTF-8 character and gives out only whole text.
#[derive(Debug, Default)]
pub(crate) struct Utf8Joiner {
    pending: Vec<u8>,
}

impl Utf8Joiner {
    /// The complete text so far; an incomplete character waits for the next piece.
    pub(crate) fn push(&mut self, piece: &[u8]) -> String {
        self.pending.extend_from_slice(piece);
        let complete = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => self.pending.len(),
        };
        let rest = self.pending.split_off(complete);
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending = rest;
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_rest_of_a_split_character() {
        let mut joiner = Utf8Joiner::default();
        let crown = "♔".as_bytes();
        assert_eq!(joiner.push(b"a"), "a");
        assert_eq!(joiner.push(crown.get(..1).unwrap_or_default()), "");
        assert_eq!(joiner.push(crown.get(1..).unwrap_or_default()), "♔");
        assert_eq!(joiner.push(&[0xff]), "\u{fffd}");
    }
}
