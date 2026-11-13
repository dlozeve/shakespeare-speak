//! Canonicalisation used to match candidate phrases against the corpus.
//!
//! Both the corpus and every candidate phrase are folded with the same
//! function, so a hit is a genuine substring match on the folded text.  The
//! folded form is never shown to the user: quotations are always re-extracted
//! from the original corpus lines.

/// Fold a string into the matching form: lowercase ASCII letters and digits,
/// single spaces between words, no punctuation, accents removed.
///
/// Two apostrophe rules matter:
///
/// * Possessives and contractions simply lose the apostrophe, so `'tis` folds
///   to `tis` and `we'll` to `well` — which is what a model will usually write.
/// * An apostrophe before a word-final `d` after a consonant is an elided `e`
///   (`colour'd`, `arm'd`, `banish'd`), so it folds to `ed`.  Without this,
///   `coloured` would never match `colour'd`.
pub fn fold(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    walk(input, |_, piece| out.push_str(piece));
    out
}

/// Map a byte offset in `fold(line)` back to a byte offset in `line`.
///
/// Used to recover the exact span of a match: the corpus is searched in folded
/// form, but quotations are sliced out of the original text.  Returns
/// `line.len()` when `folded_offset` is at or past the end.
pub fn fold_offset_to_original(line: &str, folded_offset: usize) -> usize {
    let mut emitted = 0usize;
    let mut found = None;

    walk(line, |byte_index, piece| {
        if found.is_some() {
            return;
        }
        if folded_offset < emitted + piece.len() {
            found = Some(byte_index);
        }
        emitted += piece.len();
    });

    found.unwrap_or(line.len())
}

/// Walk `input`, calling `emit(original_byte_index, folded_piece)` for every
/// piece of folded output.  This is the single source of truth for folding, so
/// `fold` and `fold_offset_to_original` can never drift apart.
fn walk(input: &str, mut emit: impl FnMut(usize, &str)) {
    let chars: Vec<char> = input.chars().collect();
    let offsets: Vec<usize> = input.char_indices().map(|(index, _)| index).collect();

    let mut last_emitted: Option<char> = None;
    let mut pending_space = false;
    let mut started = false;

    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let at = offsets[index];
        let mut advance = 1;
        let mut buffer = [0u8; 4];

        let piece: Option<&str> = match ch {
            _ if is_apostrophe(ch) => {
                let previous = last_emitted;
                let next = chars.get(index + 1).copied();
                let after = chars.get(index + 2).copied();

                let previous_is_consonant = matches!(
                    previous,
                    Some(c) if c.is_ascii_lowercase() && !"aeiou".contains(c)
                );
                let ends_word = after.is_none_or(|c| !c.is_alphanumeric());

                // `colour'd` -> `coloured`
                if previous_is_consonant && next == Some('d') && ends_word {
                    advance = 2;
                    Some("ed")
                } else {
                    None
                }
            }
            'æ' | 'Æ' => Some("ae"),
            'œ' | 'Œ' => Some("oe"),
            'ß' => Some("ss"),
            _ => match accent_base(ch) {
                Some(base) => Some(base.encode_utf8(&mut buffer)),
                None if ch.is_ascii_alphanumeric() => {
                    Some(ch.to_ascii_lowercase().encode_utf8(&mut buffer))
                }
                None => None,
            },
        };

        match piece {
            Some(text) => {
                if pending_space && started {
                    emit(at, " ");
                }
                pending_space = false;
                started = true;
                emit(at, text);
                last_emitted = text.chars().last();
            }
            None => {
                // A dropped apostrophe is invisible; anything else separates words.
                if !is_apostrophe(ch) {
                    pending_space = true;
                }
            }
        }

        index += advance;
    }
}

fn is_apostrophe(c: char) -> bool {
    matches!(
        c,
        '\'' | '\u{2018}' | '\u{2019}' | '\u{02BC}' | '`' | '\u{00B4}'
    )
}

/// Fold the accented letters that actually occur in this edition.
fn accent_base(c: char) -> Option<char> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
        'ç' | 'Ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
        'ñ' | 'Ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{fold, fold_offset_to_original};

    #[test]
    fn folds_typography_and_punctuation() {
        assert_eq!(fold("At one fell swoop?"), "at one fell swoop");
        assert_eq!(fold("  At   one\tfell  swoop  "), "at one fell swoop");
        assert_eq!(fold("beauty’s rose"), "beautys rose");
        assert_eq!(fold("one—two"), "one two");
        assert_eq!(fold("self-substantial"), "self substantial");
    }

    #[test]
    fn drops_apostrophes_so_elisions_match() {
        assert_eq!(fold("’Tis but a scratch"), "tis but a scratch");
        assert_eq!(fold("o’er the ramparts"), "oer the ramparts");
        assert_eq!(fold("we’ll"), "well");
        assert_eq!(fold("ta’en"), "taen");
    }

    #[test]
    fn expands_elided_e_before_final_d() {
        assert_eq!(fold("colour’d"), "coloured");
        assert_eq!(fold("arm’d"), "armed");
        assert_eq!(fold("banish’d"), "banished");
        assert_eq!(fold("purple-colour’d face"), "purple coloured face");
        // A pronoun contraction is not an elided "e".
        assert_eq!(fold("he’d"), "hed");
        assert_eq!(fold("we’d"), "wed");
        // Not word-final, so not the elision pattern.
        assert_eq!(fold("don’t"), "dont");
    }

    #[test]
    fn folds_accents_and_ligatures() {
        assert_eq!(fold("Dramatis Personæ"), "dramatis personae");
        assert_eq!(fold("café"), "cafe");
        assert_eq!(fold("manœuvre"), "manoeuvre");
    }

    #[test]
    fn empty_and_punctuation_only() {
        assert_eq!(fold(""), "");
        assert_eq!(fold("?!—…"), "");
    }

    #[test]
    fn maps_folded_offsets_back_to_the_original() {
        let line = "and perfect image of life indeed. The better part of valour is";
        let folded = fold(line);
        let at = folded.find("better part").unwrap();
        let original = fold_offset_to_original(line, at);
        assert_eq!(&line[original..], "better part of valour is");

        // A leading apostrophe is dropped, so the offset must skip it.
        let line = "’Tis but a scratch";
        let folded = fold(line);
        assert_eq!(
            fold_offset_to_original(line, folded.find("scratch").unwrap()),
            13
        );
        assert_eq!(&line[13..], "scratch");

        // Past the end clamps to the line length.
        assert_eq!(fold_offset_to_original(line, 999), line.len());
    }

    #[test]
    fn offsets_survive_elision_expansion() {
        let line = "Even as the sun with purple-colour’d face";
        let folded = fold(line);
        let at = folded.find("face").unwrap();
        let original = fold_offset_to_original(line, at);
        assert_eq!(&line[original..], "face");
    }
}
