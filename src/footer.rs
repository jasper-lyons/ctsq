//! The footer every command prints after output that dropped something: a count
//! line, then one `hidden:` line naming each reason, its count and the flag that
//! brings the hidden part back. It is always the last thing on stdout. Output
//! that hid nothing gets no footer, so it is unchanged.
//!
//! Every command words its footer through `footer`, so the wording lives here.
//! `--max-matches` is a process-wide cap kept here too: a command asks `take`
//! before it prints a row, and `settle` copies what was cut into the `Hidden`.

use crate::files::Hide;

/// Matches cut by `--max-matches` (BUILD.md A18).
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
    format!("{} {}\n{}", shown, plural(noun, shown), hidden_lines(hidden))
}

/// The `hidden:` lines, one per reason in the order tests, excluded paths, over
/// the cap, each ending in a newline. Empty when nothing was hidden.
pub fn hidden_lines(hidden: &Hidden) -> String {
    let mut out = String::new();
    if hidden.tests > 0 {
        out.push_str(&format!("hidden: {} in tests (--tests)\n", hidden.tests));
    }
    if hidden.excluded > 0 {
        out.push_str(&format!("hidden: {} in excluded paths (drop --exclude)\n", hidden.excluded));
    }
    if let Some(m) = hidden.over_max.as_ref().filter(|m| m.count > 0) {
        out.push_str(&format!(
            "hidden: {} over --max-matches {} in {} {} (--max-matches {})\n",
            m.count,
            m.max,
            m.files,
            plural("file", m.files),
            m.suggest
        ));
    }
    out
}

/// What a call says on stderr when every match was hidden: `phrase` is the
/// command's miss wording up to the hiding reason (`no caller to X`, `no match
/// for Q`), then `outside tests`, where it looked and how many files, then the
/// `hidden:` lines. Stdout gets nothing (BUILD.md A20).
pub fn nothing_shown(phrase: &str, where_: &str, files: usize, hidden: &Hidden) -> String {
    let mut reasons = Vec::new();
    if hidden.tests > 0 {
        reasons.push("tests");
    }
    if hidden.excluded > 0 {
        reasons.push("excluded paths");
    }
    let outside = if reasons.is_empty() { String::new() } else { format!(" outside {}", reasons.join(" and ")) };
    format!("{}{} in {} ({} {})\n{}", phrase, outside, where_, files, plural("file", files), hidden_lines(hidden))
}

/// The `--max-matches` cap: how many rows were let through, and what was cut.
#[derive(Debug)]
pub struct Cap {
    max: Option<usize>,
    taken: usize,
    cut: usize,
    files: std::collections::BTreeSet<String>,
}

impl Cap {
    pub const fn with(max: Option<usize>) -> Cap {
        Cap { max, taken: 0, cut: 0, files: std::collections::BTreeSet::new() }
    }

    /// Whether a row in `file` may print. Past the cap it is counted as cut.
    pub fn take(&mut self, file: &str) -> bool {
        match self.max {
            Some(m) if self.taken >= m => {
                self.cut += 1;
                self.files.insert(file.to_string());
                false
            }
            Some(_) => {
                self.taken += 1;
                true
            }
            None => true,
        }
    }

    /// Count `n` rows in `file` as cut without asking.
    pub fn cut(&mut self, file: &str, n: usize) {
        if self.max.is_some() && n > 0 {
            self.cut += n;
            self.files.insert(file.to_string());
        }
    }

    /// The over-cap reason, when something was cut.
    pub fn over(&self) -> Option<OverMax> {
        let max = self.max?;
        (self.cut > 0).then(|| OverMax {
            count: self.cut,
            max,
            files: self.files.len(),
            suggest: self.taken + self.cut,
        })
    }
}

static CAP: std::sync::Mutex<Cap> = std::sync::Mutex::new(Cap::with(None));

/// Set the process-wide cap from `--max-matches`.
pub fn set_max(max: Option<usize>) {
    *CAP.lock().unwrap() = Cap::with(max);
}

/// Whether a row in `file` may print under the process-wide cap.
pub fn take(file: &str) -> bool {
    CAP.lock().unwrap().take(file)
}

/// Count `n` rows in `file` as cut under the process-wide cap.
pub fn cut(file: &str, n: usize) {
    CAP.lock().unwrap().cut(file, n)
}

/// Copy what the process-wide cap cut into `hidden`, ahead of the footer.
pub fn settle(hidden: &mut Hidden) {
    if let Some(o) = CAP.lock().unwrap().over() {
        hidden.over_max = Some(o);
    }
}

/// The last lines of a command's output. With `count` (`--scope` asks for it)
/// the count line always prints, merged into the footer when something was
/// hidden, so it never prints twice.
pub fn tail(shown: usize, noun: &str, hidden: &Hidden, count: bool) -> String {
    if hidden.any() || !count {
        footer(shown, noun, hidden)
    } else {
        format!("{} {}\n", shown, plural(noun, shown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_line_is_asked_for_and_never_doubled() {
        assert_eq!(tail(2, "definition", &Hidden::default(), true), "2 definitions\n");
        assert_eq!(tail(2, "definition", &Hidden::default(), false), "");
        let mut h = Hidden::default();
        h.count(Hide::Test, 1);
        assert_eq!(tail(2, "match", &h, true), "2 matches\nhidden: 1 in tests (--tests)\n");
    }

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
            "2 matches\nhidden: 6 in tests (--tests)\nhidden: 31 over --max-matches 2 in 4 files (--max-matches 40)\n"
        );
    }

    #[test]
    fn each_reason_names_its_count_and_flag_in_a_fixed_order() {
        let h = Hidden { tests: 1, excluded: 5, over_max: Some(OverMax { count: 2, max: 10, files: 1, suggest: 12 }) };
        assert_eq!(
            footer(1, "match", &h),
            "1 match\nhidden: 1 in tests (--tests)\nhidden: 5 in excluded paths (drop --exclude)\nhidden: 2 over --max-matches 10 in 1 file (--max-matches 12)\n"
        );
    }

    #[test]
    fn the_cap_lets_n_rows_through_and_counts_the_rest_by_file() {
        let mut c = Cap::with(Some(2));
        assert!(c.take("a.rs") && c.take("a.rs"));
        assert!(!c.take("b.rs") && !c.take("c.rs") && !c.take("b.rs"));
        assert_eq!(c.over(), Some(OverMax { count: 3, max: 2, files: 2, suggest: 5 }));
        assert_eq!(Cap::with(None).over(), None);
        let mut roomy = Cap::with(Some(9));
        assert!(roomy.take("a.rs"));
        assert_eq!(roomy.over(), None);
    }

    #[test]
    fn everything_hidden_reads_as_a_stderr_report_with_the_hidden_lines() {
        let mut h = Hidden::default();
        h.count(Hide::Test, 3);
        assert_eq!(
            nothing_shown("no caller to run", "src/", 4, &h),
            "no caller to run outside tests in src/ (4 files)\nhidden: 3 in tests (--tests)\n"
        );
        h.count(Hide::Excluded, 1);
        assert!(nothing_shown("no match for id#x", "src/", 1, &h)
            .starts_with("no match for id#x outside tests and excluded paths in src/ (1 file)\n"));
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
