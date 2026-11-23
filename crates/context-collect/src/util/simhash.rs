//! A 64-bit SimHash over words: texts that differ in a few words get fingerprints that differ in
//! a few bits.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn word_hash(word: &str) -> u64 {
    word.chars()
        .flat_map(char::to_lowercase)
        .fold(FNV_OFFSET, |hash, character| {
            (hash ^ u64::from(character)).wrapping_mul(FNV_PRIME)
        })
}

pub(crate) fn simhash<'a>(lines: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut weights = [0i32; 64];
    let words = lines
        .into_iter()
        .flat_map(|line| line.split(|character: char| !character.is_alphanumeric()))
        .filter(|word| !word.is_empty());
    for word in words {
        let hash = word_hash(word);
        for (bit, weight) in weights.iter_mut().enumerate() {
            *weight += if hash >> bit & 1 == 1 { 1 } else { -1 };
        }
    }
    weights
        .iter()
        .enumerate()
        .filter(|(_, weight)| **weight > 0)
        .fold(0, |fingerprint, (bit, _)| fingerprint | 1 << bit)
}

pub(crate) const fn hamming_distance(left: u64, right: u64) -> u32 {
    (left ^ right).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAGRAPH: &str = "The compositor bridge binds the context protocol on its own thread \
        and turns every event into an observation that the pipeline judges, segments and keeps \
        for the working memory of the engine while the user works on the damage tracker report";

    #[test]
    fn equal_text_gives_equal_fingerprints() {
        assert_eq!(simhash([PARAGRAPH]), simhash([PARAGRAPH]));
    }

    #[test]
    fn case_and_punctuation_do_not_count() {
        let shouted = PARAGRAPH.to_uppercase().replace(' ', " ,");
        assert_eq!(simhash([PARAGRAPH]), simhash([shouted.as_str()]));
    }

    #[test]
    fn a_small_edit_stays_near() {
        let edited = PARAGRAPH.replace("damage tracker", "damage trackers");
        assert!(hamming_distance(simhash([PARAGRAPH]), simhash([edited.as_str()])) <= 10);
    }

    #[test]
    fn different_text_lands_far() {
        let other = "Quarterly budget spreadsheet with revenue forecasts for the northern \
            region, salaries, office rent, travel and a pivot table per month";
        assert!(hamming_distance(simhash([PARAGRAPH]), simhash([other])) > 10);
    }
}
