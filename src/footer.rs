//! The footer every command prints after output that dropped something: a count
//! line, then one `hidden:` line naming each reason, its count and the flag that
//! brings the hidden part back. It is always the last thing on stdout. Output
//! that hid nothing gets no footer, so it is unchanged.
//!
//! Every command words its footer through `footer`, so the wording lives here.

use crate::files::Hide;

/// Matches cut by a cap such as `--max`. Not set by any command yet: the flag
/// is held (BUILD.md A14), the reason is here so adding it is small.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverMax {
    /// Matches not shown.
    pub count: usize,
    /// The cap in force.
    pub max: usize,
    /// Files the cut matches sit in.
    pub files: usize,
    /// The cap that would show them all.
    pub suggest: usize,
}

/// What one run hid, by reason.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hidden {
    /// Matches in test files or test scopes.
    pub tests: usize,
    /// Matches in paths an `--exclude` glob named.
    pub excluded: usize,
    pub over_max: Option<OverMax>,
}

impl Hidden {
    pub fn any(&self) -> bool {
        self.tests > 0 || self.excluded > 0 || self.over_max.as_ref().is_some_and(|m| m.count > 0)
    }

    /// Count `n` hidden for `why`.
    pub fn count(&mut self, why: Hide, n: usize) {
        match why {
            Hide::Test => self.tests += n,
            Hide::Excluded => self.excluded += n,
        }
    }
}

/// `noun` in the plural when `n` is not one: match, matches. entry, entries.
pub fn plural(noun: &str, n: usize) -> String {
    if n == 1 {
        return noun.to_string();
    }
    let consonant_y = noun.len() > 1
        && noun.ends_with('y')
        && !matches!(noun.as_bytes()[noun.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u');
    if consonant_y {
        format!("{}ies", &noun[..noun.len() - 1])
    } else if noun.ends_with("ch") || noun.ends_with("sh") || noun.ends_with('s') || noun.ends_with('x') {
        format!("{}es", noun)
    } else {
        format!("{}s", noun)
    }
}

/// The footer, ending in a newline, or an empty string when nothing was hidden.
/// `shown` is how many `noun`s the command printed.
pub fn footer(shown: usize, noun: &str, hidden: &Hidden) -> String {
    if !hidden.any() {
        return String::new();
    }
    let mut reasons = Vec::new();
    if hidden.tests > 0 {
        reasons.push(format!("{} in tests (--tests)", hidden.tests));
    }
    if hidden.excluded > 0 {
        reasons.push(format!("{} in excluded paths (drop --exclude)", hidden.excluded));
    }
    if let Some(m) = hidden.over_max.as_ref().filter(|m| m.count > 0) {
        reasons.push(format!(
            "{} over --max {} in {} {} (--max {})",
            m.count,
            m.max,
            m.files,
            plural("file", m.files),
            m.suggest
        ));
    }
    format!("{} {}\nhidden: {}\n", shown, plural(noun, shown), reasons.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_hidden_prints_nothing() {
        assert_eq!(footer(3, "match", &Hidden::default()), "");
        let zero_max = Hidden { over_max: Some(OverMax::default()), ..Default::default() };
        assert_eq!(footer(3, "match", &zero_max), "");
    }

    #[test]
    fn the_footer_has_the_output_format_shape() {
        let h = Hidden {
            tests: 6,
            excluded: 0,
            over_max: Some(OverMax { count: 31, max: 2, files: 4, suggest: 40 }),
        };
        assert_eq!(
            footer(2, "match", &h),
            "2 matches\nhidden: 6 in tests (--tests), 31 over --max 2 in 4 files (--max 40)\n"
        );
    }

    #[test]
    fn each_reason_names_its_count_and_flag_in_a_fixed_order() {
        let h = Hidden { tests: 1, excluded: 5, over_max: Some(OverMax { count: 2, max: 10, files: 1, suggest: 12 }) };
        assert_eq!(
            footer(1, "match", &h),
            "1 match\nhidden: 1 in tests (--tests), 5 in excluded paths (drop --exclude), 2 over --max 10 in 1 file (--max 12)\n"
        );
    }

    #[test]
    fn every_match_hidden_still_gives_a_count_line() {
        let mut h = Hidden::default();
        h.count(Hide::Test, 4);
        assert_eq!(footer(0, "match", &h), "0 matches\nhidden: 4 in tests (--tests)\n");
    }

    #[test]
    fn nouns_take_their_plural() {
        assert_eq!(plural("entry", 2), "entries");
        assert_eq!(plural("entry", 1), "entry");
        assert_eq!(plural("match", 0), "matches");
        assert_eq!(plural("caller", 3), "callers");
        assert_eq!(plural("file", 2), "files");
        assert_eq!(plural("definition", 2), "definitions");
        assert_eq!(plural("key", 2), "keys");
    }

    #[test]
    fn the_output_has_no_semicolons_or_em_dashes() {
        let h = Hidden { tests: 1, excluded: 1, over_max: Some(OverMax { count: 1, max: 1, files: 1, suggest: 2 }) };
        let s = footer(1, "match", &h);
        assert!(!s.contains(';') && !s.contains('\u{2014}'), "{}", s);
    }
}
