//! Exact text boundaries from Qwen3-ASR forced-prefix decoding.
//! Offsets are UTF-8 bytes, never word counts or confidence estimates.

use crate::text_filters::TextFilters;

/// Invalid offsets carry no guarantee. Never turn malformed metadata into
/// a claim that the whole hypothesis is frozen.
pub fn boundary(text: &str, bytes: usize) -> usize {
    if bytes <= text.len() && text.is_char_boundary(bytes) {
        bytes
    } else {
        0
    }
}

/// Map the raw forced prefix into the displayed (filtered) text. Whole-text
/// filtering can reject a hypothesis or change Unicode byte lengths. Claim
/// only a prefix that survives the identical display transform.
pub fn filtered(
    text: &str,
    frozen_bytes: usize,
    filters: &TextFilters,
    language: Option<&str>,
) -> (String, usize) {
    let end = boundary(text, frozen_bytes);
    let shown = filters.apply(text, language);
    let prefix = filters.apply(&text[..end], language);
    let frozen = crate::ghost::frozen_len(&shown, &prefix);
    (shown, frozen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_and_invalid_boundaries() {
        assert_eq!(boundary("blåbær", 4), 4);
        assert_eq!(boundary("blåbær", 3), 0);
        assert_eq!(boundary("blåbær", 99), 0);
        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: true,
            strict_alphabet: false,
            ..Default::default()
        };
        let (shown, frozen) = filtered("İ! BLUE tail", "İ! BLUE".len(), &filters, None);
        assert_eq!(shown, "i\u{307} blue tail");
        assert_eq!(&shown[..frozen], "i\u{307} blue");
    }

    #[test]
    fn rejected_hypothesis_has_no_frozen_text() {
        assert_eq!(
            filtered("hello 你好世界世界", 5, &TextFilters::default(), Some("en")),
            (String::new(), 0)
        );
    }
}
