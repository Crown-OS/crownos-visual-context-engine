/// The labels of a PP-OCR recogniser: the CTC blank, the dictionary lines, then a space.
#[derive(Debug)]
pub(super) struct Labels {
    labels: Vec<String>,
}

impl Labels {
    pub(super) fn from_dictionary(dictionary: &str) -> Self {
        let labels = std::iter::once(String::new())
            .chain(dictionary.lines().map(str::to_owned))
            .chain(std::iter::once(" ".to_owned()))
            .collect();
        Self { labels }
    }

    pub(super) const fn len(&self) -> usize {
        self.labels.len()
    }

    /// Greedy CTC decoding of `steps` rows of `classes` probabilities: the best label per
    /// step, repeats merged, blanks dropped. The confidence is the mean probability of the
    /// kept labels.
    pub(super) fn decode(&self, probabilities: &[f32], classes: usize) -> (String, f32) {
        let mut text = String::new();
        let (mut confidence_sum, mut kept) = (0.0, 0u32);
        let mut previous = 0;
        for step in probabilities.chunks_exact(classes.max(1)) {
            let (best, probability) = step
                .iter()
                .copied()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(&right.1))
                .unwrap_or((0, 0.0));
            if best != 0
                && best != previous
                && let Some(label) = self.labels.get(best)
            {
                text.push_str(label);
                confidence_sum += probability;
                kept += 1;
            }
            previous = best;
        }
        let confidence = if kept == 0 {
            0.0
        } else {
            confidence_sum / kept as f32
        };
        (text, confidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_repeats_and_drops_blanks() {
        let labels = Labels::from_dictionary("a\nb");
        let one_hot = |label: usize| {
            let mut step = vec![0.0; 4];
            if let Some(value) = step.get_mut(label) {
                *value = 0.9;
            }
            step
        };
        let probabilities: Vec<f32> = [1, 1, 0, 1, 2, 3, 2]
            .into_iter()
            .flat_map(one_hot)
            .collect();
        let (text, confidence) = labels.decode(&probabilities, 4);
        assert_eq!(text, "aab b");
        assert!((confidence - 0.9).abs() < 1e-6);
    }
}
