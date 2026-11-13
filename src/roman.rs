//! Roman numeral parsing and formatting for act/scene numbers.

const SYMBOLS: [(u32, &str); 13] = [
    (1000, "m"),
    (900, "cm"),
    (500, "d"),
    (400, "cd"),
    (100, "c"),
    (90, "xc"),
    (50, "l"),
    (40, "xl"),
    (10, "x"),
    (9, "ix"),
    (5, "v"),
    (4, "iv"),
    (1, "i"),
];

/// Parse an uppercase roman numeral such as `IV` or `XXVII`.
pub fn parse(text: &str) -> Option<u32> {
    if text.is_empty() {
        return None;
    }

    let mut total: u32 = 0;
    let mut previous: u32 = 0;

    for ch in text.chars().rev() {
        let value = match ch {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1000,
            _ => return None,
        };
        if value < previous {
            total = total.checked_sub(value)?;
        } else {
            total = total.checked_add(value)?;
            previous = value;
        }
    }

    if total == 0 {
        None
    } else {
        Some(total)
    }
}

/// Format a number as a lowercase roman numeral (`4` -> `iv`).
pub fn to_lower(mut n: u32) -> String {
    let mut out = String::new();
    for (value, symbol) in SYMBOLS {
        while n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{parse, to_lower};

    #[test]
    fn parses_romans() {
        assert_eq!(parse("I"), Some(1));
        assert_eq!(parse("IV"), Some(4));
        assert_eq!(parse("IX"), Some(9));
        assert_eq!(parse("XIV"), Some(14));
        assert_eq!(parse("XXVII"), Some(27));
        assert_eq!(parse("XLII"), Some(42));
        assert_eq!(parse(""), None);
        assert_eq!(parse("ABC"), None);
    }

    #[test]
    fn formats_romans() {
        assert_eq!(to_lower(1), "i");
        assert_eq!(to_lower(3), "iii");
        assert_eq!(to_lower(4), "iv");
        assert_eq!(to_lower(14), "xiv");
        assert_eq!(to_lower(42), "xlii");
    }

    #[test]
    fn round_trips() {
        for n in 1..=60 {
            assert_eq!(parse(&to_lower(n).to_uppercase()), Some(n));
        }
    }
}
