//! The local corpus: one text file per work, plus the structure needed to turn
//! a line number into a citation.
//!
//! Nothing in this module talks to a model.  Every quotation and every act /
//! scene number the tool prints is derived here, from the text on disk.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::fold::{fold, fold_offset_to_original};
use crate::roman;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Act(u32),
    Scene(u32),
    Sonnet(u32),
}

#[derive(Debug, Clone, Copy)]
pub struct Marker {
    /// 0-based line index in the work.
    pub line: usize,
    pub kind: MarkerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Has act/scene structure.
    Play,
    /// The sonnet sequence.
    Sonnets,
    /// A single poem.
    Poem,
}

#[derive(Debug)]
pub struct Work {
    /// File stem, e.g. `the-tragedy-of-macbeth`.
    pub slug: String,
    /// Heading as it appears in the source, e.g. `THE TRAGEDY OF MACBETH`.
    pub title: String,
    pub path: PathBuf,
    pub lines: Vec<String>,
    /// Folded text of the whole work, lines joined by a single space.
    pub folded: String,
    /// Byte offset in `folded` where each line starts.
    pub line_offsets: Vec<usize>,
    /// First line that is part of the work itself (past title/contents/cast).
    pub body_start: usize,
    pub markers: Vec<Marker>,
    pub kind: Kind,
}

/// Where a line sits in the work's structure.
#[derive(Debug, Clone, Copy, Default)]
pub struct Placement {
    pub act: Option<u32>,
    pub scene: Option<u32>,
    pub sonnet: Option<u32>,
}

/// Where a phrase was found: an inclusive line range plus the exact byte span
/// within the first and last line, so the quotation can be sliced precisely
/// instead of showing whole wrapped lines.
#[derive(Debug, Clone, Copy)]
pub struct Occurrence {
    pub start_line: usize,
    pub end_line: usize,
    /// Byte offset within `start_line` where the quotation begins.
    pub start_col: usize,
    /// Byte offset within `end_line` where the quotation ends (exclusive).
    pub end_col: usize,
}

/// A single verified occurrence of a phrase.
#[derive(Debug)]
pub struct Hit<'a> {
    pub work: &'a Work,
    pub occurrence: Occurrence,
}

/// Where the corpus came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Compiled into the binary.
    Embedded,
    /// Read from a directory at startup.
    Directory,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Embedded => "embedded",
            Source::Directory => "directory",
        }
    }
}

pub struct Corpus {
    pub works: Vec<Work>,
    pub source: Source,
}

impl Corpus {
    /// Load every `*.txt` file in `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            bail!("corpus directory not found: {}", dir.display());
        }

        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
            .collect();
        paths.sort();

        if paths.is_empty() {
            bail!("no *.txt files in {}", dir.display());
        }

        let mut works = Vec::with_capacity(paths.len());
        for path in paths {
            works.push(Work::load(&path)?);
        }

        Ok(Corpus {
            works,
            source: Source::Directory,
        })
    }

    /// Build the corpus from the text compiled into the binary.
    pub fn embedded() -> Result<Self> {
        let mut works = Vec::with_capacity(crate::embedded::WORKS.len());
        for (slug, text) in crate::embedded::WORKS {
            let path = PathBuf::from(format!("data/works/{slug}.txt"));
            works.push(Work::from_text(slug.to_string(), path, text)?);
        }

        if works.is_empty() {
            bail!("the embedded corpus is empty");
        }

        Ok(Corpus {
            works,
            source: Source::Embedded,
        })
    }

    /// Find every occurrence of an already-folded phrase, in corpus order.
    pub fn find(&self, folded_needle: &str) -> Vec<Hit<'_>> {
        let mut hits = Vec::new();
        for work in &self.works {
            for occurrence in work.occurrences(folded_needle) {
                hits.push(Hit { work, occurrence });
            }
        }
        hits
    }
}

impl Work {
    fn load(path: &Path) -> Result<Self> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

        let slug = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();

        Work::from_text(slug, path.to_path_buf(), &raw)
    }

    /// Parse one work from its text. `path` is only used for display.
    fn from_text(slug: String, path: PathBuf, raw: &str) -> Result<Self> {
        let lines: Vec<String> = raw.lines().map(|line| line.to_string()).collect();

        let title = lines
            .iter()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_else(|| path.display().to_string());

        // The work proper starts after the front matter.  Every play has a
        // "Dramatis Personæ" heading; Richard II is the one play whose cast
        // list has no heading, but it still has a "SCENE:" location line.
        let body_start = lines
            .iter()
            .position(|line| {
                let trimmed = line.trim().to_lowercase();
                trimmed.starts_with("dramatis person")
            })
            .or_else(|| {
                lines
                    .iter()
                    .position(|line| line.trim().starts_with("SCENE:"))
            })
            .unwrap_or(1)
            .min(lines.len().saturating_sub(1));

        let mut markers = Vec::new();
        for (index, line) in lines.iter().enumerate().skip(body_start + 1) {
            let trimmed = line.trim();
            if let Some(kind) = parse_marker(trimmed) {
                markers.push(Marker { line: index, kind });
            }
        }

        let kind = if markers.iter().any(|m| matches!(m.kind, MarkerKind::Act(_))) {
            Kind::Play
        } else {
            // No act structure: either the sonnet sequence (many centred
            // numbers) or a single poem.
            let sonnets: Vec<Marker> = lines
                .iter()
                .enumerate()
                .skip(body_start + 1)
                .filter_map(|(index, line)| {
                    let trimmed = line.trim();
                    let is_centred = line.len() > trimmed.len() && trimmed.len() < 5;
                    let number: u32 = trimmed.parse().ok()?;
                    is_centred.then_some(Marker {
                        line: index,
                        kind: MarkerKind::Sonnet(number),
                    })
                })
                .collect();

            if sonnets.len() > 50 {
                markers = sonnets;
                Kind::Sonnets
            } else {
                markers.clear();
                Kind::Poem
            }
        };

        // Fold the whole work once; offsets let us map a match back to lines.
        let mut folded = String::with_capacity(raw.len());
        let mut line_offsets = Vec::with_capacity(lines.len());
        for line in &lines {
            line_offsets.push(folded.len());
            let folded_line = fold(line);
            if folded_line.is_empty() {
                continue;
            }
            folded.push_str(&folded_line);
            folded.push(' ');
        }

        Ok(Work {
            slug,
            title,
            path,
            lines,
            folded,
            line_offsets,
            body_start,
            markers,
            kind,
        })
    }

    /// Line index containing a byte offset into `folded`.
    fn line_at(&self, offset: usize) -> usize {
        match self.line_offsets.partition_point(|&start| start <= offset) {
            0 => 0,
            count => count - 1,
        }
    }

    /// All occurrences of a folded phrase.
    ///
    /// Matches that fall inside front matter, stage directions, speaker labels
    /// or structural headings are discarded: those are not quotations.
    pub fn occurrences(&self, folded_needle: &str) -> Vec<Occurrence> {
        let mut found = Vec::new();
        if folded_needle.is_empty() {
            return found;
        }

        let from = self.line_offsets.get(self.body_start).copied().unwrap_or(0);

        let mut cursor = from;
        while cursor < self.folded.len() {
            let Some(relative) = self.folded[cursor..].find(folded_needle) else {
                break;
            };
            let start = cursor + relative;
            let end = start + folded_needle.len();
            let start_line = self.line_at(start);
            let end_line = self.line_at(end.saturating_sub(1));

            if self.is_quotable(start_line) {
                let start_col = fold_offset_to_original(
                    &self.lines[start_line],
                    start - self.line_offsets[start_line],
                );
                let end_col = fold_offset_to_original(
                    &self.lines[end_line],
                    end - self.line_offsets[end_line],
                );
                found.push(Occurrence {
                    start_line,
                    end_line,
                    start_col,
                    end_col,
                });
            }

            cursor = start + folded_needle.len().max(1);
        }

        found
    }

    /// The contiguous run of quotable lines around `line`.
    ///
    /// Speeches in this edition are separated by blank lines, and a speaker
    /// label, stage direction or structural heading always breaks the run, so
    /// this is the passage the line belongs to.
    pub fn speech_block(&self, line: usize) -> (usize, usize) {
        let mut start = line.min(self.lines.len().saturating_sub(1));
        while start > self.body_start && self.is_quotable(start - 1) {
            start -= 1;
        }

        let mut end = start;
        while end + 1 < self.lines.len() && self.is_quotable(end + 1) {
            end += 1;
        }

        (start, end)
    }

    /// Widen a match to a window of at most `max_lines` whole lines from its
    /// own speech.
    ///
    /// The result always covers whole lines, never a slice of one: a match on
    /// `help me hence` in `Help me hence, ho!` returns the whole verse.  The
    /// window ends at the match, so a line that closes a speech brings its
    /// lead-up with it, while a line that opens a speech brings the lines that
    /// follow.
    ///
    /// A match that already spans more lines than `max_lines` is left whole
    /// rather than truncated.
    pub fn expand(&self, occurrence: &Occurrence, max_lines: usize) -> Occurrence {
        let budget = max_lines.max(1);
        let (block_start, block_end) = self.speech_block(occurrence.end_line);

        let wanted = occurrence.end_line.saturating_sub(budget - 1);
        let start = block_start.max(wanted).min(occurrence.start_line);
        let end = block_end.min(start + budget - 1).max(occurrence.end_line);

        // Verse lines are the unit of sense, so a line window is already a
        // complete quotation.  Prose is wrapped mid-sentence, so a line window
        // would start and stop in the middle of one; align it to sentences
        // instead, as long as that does not run away with the answer.
        if self.is_prose_block(start, end) {
            let (start_line, start_col) =
                self.sentence_start(occurrence.start_line, occurrence.start_col);
            let (end_line, end_col) = self.sentence_end(occurrence.end_line, occurrence.end_col);

            if end_line.saturating_sub(start_line) < budget + 6 {
                return Occurrence {
                    start_line,
                    end_line,
                    start_col,
                    end_col,
                };
            }
        }

        Occurrence {
            start_line: start,
            end_line: end,
            start_col: 0,
            end_col: self.lines[end].len(),
        }
    }

    /// A speech whose lines are long and mostly unpunctuated is wrapped prose,
    /// not verse.
    fn is_prose_block(&self, start: usize, end: usize) -> bool {
        let lines = end - start + 1;
        if lines < 2 {
            return false;
        }

        let wrapped = (start..=end)
            .filter(|&line| {
                let text = self.lines[line].trim_end();
                text.len() >= 60 && !text.ends_with(['.', '!', '?'])
            })
            .count();

        wrapped * 2 > lines
    }

    /// Walk back to just after the previous sentence end.
    fn sentence_start(&self, mut line: usize, mut col: usize) -> (usize, usize) {
        let (block_start, _) = self.speech_block(line);
        loop {
            let text = &self.lines[line];
            let head = &text[..col.min(text.len())];
            if let Some((index, _)) = head
                .char_indices()
                .rev()
                .find(|(_, c)| matches!(c, '.' | '!' | '?'))
            {
                return (line, index + 1);
            }
            if line == block_start {
                return (line, 0);
            }
            line -= 1;
            col = self.lines[line].len();
        }
    }

    /// Walk forward to just past the next sentence end.
    fn sentence_end(&self, mut line: usize, mut col: usize) -> (usize, usize) {
        let (_, block_end) = self.speech_block(line);
        loop {
            let text = &self.lines[line];
            let offset = col.min(text.len());
            let tail = &text[offset..];
            if let Some((index, _)) = tail
                .char_indices()
                .find(|(_, c)| matches!(c, '.' | '!' | '?'))
            {
                return (line, offset + index + 1);
            }
            if line == block_end {
                return (line, text.len());
            }
            line += 1;
            col = 0;
        }
    }

    fn is_quotable(&self, line: usize) -> bool {
        let Some(text) = self.lines.get(line) else {
            return false;
        };
        let trimmed = text.trim();

        if trimmed.is_empty() || parse_marker(trimmed).is_some() {
            return false;
        }
        // Stage direction.
        if trimmed.starts_with('[') {
            return false;
        }
        // Speaker label on its own line, e.g. "MACBETH." or "FIRST MURDERER."
        if is_speaker_label(trimmed) {
            return false;
        }
        true
    }

    /// Act / scene / sonnet containing a line.
    pub fn placement(&self, line: usize) -> Placement {
        let mut placement = Placement::default();
        for marker in &self.markers {
            if marker.line > line {
                break;
            }
            match marker.kind {
                MarkerKind::Act(act) => {
                    placement.act = Some(act);
                    placement.scene = None;
                }
                MarkerKind::Scene(scene) => placement.scene = Some(scene),
                MarkerKind::Sonnet(number) => placement.sonnet = Some(number),
            }
        }
        placement
    }

    /// The original text of an occurrence, one output line per source line, so
    /// verse formatting survives.  A leading speaker label is removed from the
    /// first line.
    pub fn quote_text(&self, occurrence: &Occurrence) -> String {
        let Occurrence {
            start_line,
            end_line,
            start_col,
            end_col,
        } = *occurrence;

        if start_line >= self.lines.len() {
            return String::new();
        }
        let last = end_line.min(self.lines.len() - 1);

        let mut parts = Vec::new();
        for (offset, raw) in self.lines[start_line..=last].iter().enumerate() {
            let line = start_line + offset;
            let slice = if start_line == last {
                &raw[start_col.min(raw.len())..end_col.min(raw.len())]
            } else if line == start_line {
                &raw[start_col.min(raw.len())..]
            } else if line == last {
                &raw[..end_col.min(raw.len())]
            } else {
                raw.as_str()
            };

            let mut text = slice.trim().to_string();
            if line == start_line {
                text = strip_speaker(&text);
            }
            if !text.is_empty() {
                parts.push(text);
            }
        }
        parts.join("\n")
    }

    /// Speaker of the line.
    ///
    /// This edition names the speaker either inline (`MACBETH. If it were
    /// done`) or on its own line above the speech (`MACDUFF.`), so both forms
    /// are handled.
    pub fn speaker(&self, line: usize) -> Option<String> {
        let text = self.lines.get(line)?.trim();

        // Inline: "MACBETH. If it were done ..."
        if let Some((head, _)) = text.split_once(". ") {
            let head = head.trim();
            if is_speaker_name(head) {
                return Some(head.to_string());
            }
        }

        // On its own line, immediately above the speech.
        let (block_start, _) = self.speech_block(line);
        let previous = self.lines.get(block_start.checked_sub(1)?)?.trim();
        if is_speaker_label(previous) {
            return Some(previous.trim_end_matches('.').to_string());
        }

        None
    }

    /// A readable form of the heading: `THE TRAGEDY OF MACBETH` -> `The Tragedy of Macbeth`.
    pub fn display_title(&self) -> String {
        let mut out = String::with_capacity(self.title.len());
        for (index, word) in self.title.split_whitespace().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let lower = word.to_lowercase();
            let small = matches!(
                lower.as_str(),
                "of" | "the" | "and" | "or" | "a" | "an" | "in" | "to" | "for" | "with"
            );
            if index > 0 && small {
                out.push_str(&lower);
            } else {
                let mut chars = lower.chars();
                if let Some(first) = chars.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                }
            }
        }
        out
    }
}

/// Recognise `ACT IV`, `ACT II.`, `SCENE III.` and `Scene I.`, tolerating
/// leading space.
///
/// Case matters here: this edition is inconsistent, and Pericles Act IV writes
/// `Scene I.` and `Scene II.` in title case while the rest of the file uses
/// `SCENE`.  Table-of-contents entries are excluded by `body_start`, so matching
/// either case in the body is safe.
fn parse_marker(trimmed: &str) -> Option<MarkerKind> {
    if starts_with_ignore_case(trimmed, "ACT") {
        let rest = trimmed[3..].trim().trim_end_matches('.').trim();
        return roman::parse(&rest.to_ascii_uppercase()).map(MarkerKind::Act);
    }
    if starts_with_ignore_case(trimmed, "SCENE") {
        let rest = trimmed[5..].trim();
        let number = rest.split('.').next()?.trim();
        return roman::parse(&number.to_ascii_uppercase()).map(MarkerKind::Scene);
    }
    None
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn is_speaker_label(trimmed: &str) -> bool {
    let Some(head) = trimmed.strip_suffix('.') else {
        return false;
    };
    !head.is_empty()
        && head.chars().any(|c| c.is_ascii_alphabetic())
        && head
            .chars()
            .all(|c| c.is_ascii_uppercase() || !c.is_ascii_alphabetic())
}

fn strip_speaker(text: &str) -> String {
    if let Some((head, rest)) = text.split_once(". ") {
        if is_speaker_name(head.trim()) {
            return rest.trim().to_string();
        }
    }
    text.to_string()
}

/// An all-caps character name, e.g. `MACBETH` or `FIRST MURDERER`.
fn is_speaker_name(head: &str) -> bool {
    !head.is_empty()
        && head.len() <= 40
        && head.chars().any(|c| c.is_ascii_alphabetic())
        && head
            .chars()
            .all(|c| c.is_ascii_uppercase() || !c.is_ascii_alphabetic())
}

impl fmt::Display for Placement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.act, self.scene) {
            (Some(act), Some(scene)) => write!(
                f,
                "Act {}, Scene {}",
                roman::to_lower(act).to_uppercase(),
                roman::to_lower(scene)
            ),
            (Some(act), None) => write!(f, "Act {}", roman::to_lower(act).to_uppercase()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/works")
    }

    #[test]
    fn embedded_corpus_matches_the_directory() {
        let embedded = Corpus::embedded().expect("embedded corpus");
        let directory = Corpus::load(&workspace()).expect("directory corpus");

        assert_eq!(embedded.source, Source::Embedded);
        assert_eq!(directory.source, Source::Directory);
        assert_eq!(embedded.works.len(), 44);
        assert_eq!(embedded.works.len(), directory.works.len());

        // The compiled-in copy must be identical to the files on disk, or a
        // release binary would answer differently from a debug one.
        for (compiled, on_disk) in embedded.works.iter().zip(directory.works.iter()) {
            assert_eq!(compiled.slug, on_disk.slug);
            assert_eq!(compiled.title, on_disk.title);
            assert_eq!(compiled.lines, on_disk.lines, "{}", compiled.slug);
            assert_eq!(compiled.markers.len(), on_disk.markers.len());
            assert_eq!(compiled.body_start, on_disk.body_start);
            assert_eq!(compiled.kind, on_disk.kind);
        }
    }

    #[test]
    fn embedded_corpus_answers_queries() {
        let corpus = Corpus::embedded().expect("embedded corpus");
        let hits = corpus.find(&fold("at one fell swoop"));

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].work.slug, "the-tragedy-of-macbeth");

        let placement = hits[0].work.placement(hits[0].occurrence.start_line);
        assert_eq!(placement.act, Some(4));
        assert_eq!(placement.scene, Some(3));
    }

    #[test]
    fn parses_markers() {
        assert!(matches!(parse_marker("ACT IV"), Some(MarkerKind::Act(4))));
        assert!(matches!(parse_marker("ACT II."), Some(MarkerKind::Act(2))));
        assert!(matches!(
            parse_marker("SCENE III. England."),
            Some(MarkerKind::Scene(3))
        ));
        // Title case occurs in Pericles Act IV.
        assert!(matches!(
            parse_marker("Scene I. Tarsus. An open place."),
            Some(MarkerKind::Scene(1))
        ));
        assert!(parse_marker("ACT").is_none());
        assert!(parse_marker("Nothing to see here.").is_none());
    }

    #[test]
    fn scenes_are_contiguous_within_each_act() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        for work in corpus.works.iter().filter(|w| w.kind == Kind::Play) {
            let mut current_act = 0;
            let mut expected = 1;
            for marker in &work.markers {
                match marker.kind {
                    MarkerKind::Act(act) => {
                        current_act = act;
                        expected = 1;
                    }
                    MarkerKind::Scene(scene) => {
                        assert_eq!(
                            scene, expected,
                            "{} act {} scene numbering jumps",
                            work.slug, current_act
                        );
                        expected += 1;
                    }
                    MarkerKind::Sonnet(_) => {}
                }
            }
        }
    }

    #[test]
    fn finds_fell_swoop_in_macbeth_act_four_scene_three() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let hits = corpus.find(&fold("at one fell swoop"));
        assert_eq!(hits.len(), 1, "expected exactly one occurrence");

        let hit = &hits[0];
        assert_eq!(hit.work.slug, "the-tragedy-of-macbeth");

        let placement = hit.work.placement(hit.occurrence.start_line);
        assert_eq!(placement.act, Some(4));
        assert_eq!(placement.scene, Some(3));

        let text = hit.work.quote_text(&hit.occurrence);
        assert_eq!(text, "At one fell swoop?");
        assert_eq!(
            hit.work.speaker(hit.occurrence.start_line).as_deref(),
            Some("MACDUFF")
        );
    }

    #[test]
    fn every_play_has_five_acts_and_scenes() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let plays: Vec<&Work> = corpus
            .works
            .iter()
            .filter(|work| work.kind == Kind::Play)
            .collect();
        assert_eq!(plays.len(), 38);

        for play in plays {
            let acts: Vec<u32> = play
                .markers
                .iter()
                .filter_map(|m| match m.kind {
                    MarkerKind::Act(a) => Some(a),
                    _ => None,
                })
                .collect();
            assert_eq!(acts, vec![1, 2, 3, 4, 5], "{}", play.slug);

            let scenes = play
                .markers
                .iter()
                .filter(|m| matches!(m.kind, MarkerKind::Scene(_)))
                .count();
            assert!(scenes > 0, "{} has no scenes", play.slug);
        }
    }

    #[test]
    fn every_play_has_a_cast_list_and_body() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        for work in &corpus.works {
            assert!(work.body_start > 0, "{} has no front matter", work.slug);
            assert!(
                work.body_start < work.lines.len(),
                "{} body starts past EOF",
                work.slug
            );
        }
    }

    #[test]
    fn no_work_contains_another_works_front_matter() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        for work in &corpus.works {
            let headings = work
                .lines
                .iter()
                .filter(|line| line.trim() == "Contents")
                .count();
            assert!(
                headings <= 1,
                "{} has {} Contents headings",
                work.slug,
                headings
            );
        }
    }

    #[test]
    fn sonnets_are_numbered() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let sonnets = corpus
            .works
            .iter()
            .find(|work| work.kind == Kind::Sonnets)
            .expect("sonnet sequence");
        let numbers: Vec<u32> = sonnets
            .markers
            .iter()
            .filter_map(|m| match m.kind {
                MarkerKind::Sonnet(n) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(numbers.len(), 154);
        assert_eq!(numbers.first(), Some(&1));
        assert_eq!(numbers.last(), Some(&154));
    }

    #[test]
    fn expands_a_match_to_its_speech() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let hits = corpus.find(&fold("at one fell swoop"));
        let work = hits[0].work;
        let occurrence = hits[0].occurrence;

        let whole = work.expand(&occurrence, 4);
        assert_eq!(
            work.quote_text(&whole),
            "He has no children.—All my pretty ones?\n\
             Did you say all?—O hell-kite!—All?\n\
             What, all my pretty chickens and their dam\n\
             At one fell swoop?"
        );

        // Two lines ending at the match.
        let two = work.expand(&occurrence, 2);
        assert_eq!(
            work.quote_text(&two),
            "What, all my pretty chickens and their dam\nAt one fell swoop?"
        );

        // One line is the whole verse containing the match.
        let one = work.expand(&occurrence, 1);
        assert_eq!(work.quote_text(&one), "At one fell swoop?");
    }

    #[test]
    fn expansion_keeps_the_whole_verse() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        // The match stops at "hence," but the verse is "Help me hence, ho!".
        let hits = corpus.find(&fold("help me hence"));
        let work = hits[0].work;

        let expanded = work.expand(&hits[0].occurrence, 1);
        assert_eq!(work.quote_text(&expanded), "Help me hence, ho!");

        let placement = work.placement(expanded.start_line);
        assert_eq!(placement.act, Some(2));
        assert_eq!(placement.scene, Some(3));
    }

    #[test]
    fn prose_is_aligned_to_sentences() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let hits = corpus.find(&fold("the charity of a penny cord"));
        let work = hits[0].work;

        let expanded = work.expand(&hits[0].occurrence, 2);
        assert_eq!(
            work.quote_text(&expanded),
            "O, the charity of a penny cord! It sums up thousands in a\ntrice."
        );
    }

    #[test]
    fn verse_is_not_sentence_aligned() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let hits = corpus.find(&fold("at one fell swoop"));
        let work = hits[0].work;

        // Sentence alignment would drop the first line of the speech, because
        // the line before the match ends with "?" — verse must keep it.
        let expanded = work.expand(&hits[0].occurrence, 4);
        assert_eq!(expanded.end_line - expanded.start_line + 1, 4);
        assert!(work
            .quote_text(&expanded)
            .starts_with("He has no children."));
    }

    #[test]
    fn a_long_match_is_not_truncated_by_the_budget() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        // A match spanning several lines, expanded with a budget of one line,
        // must still come back whole.
        let needle = fold("What, all my pretty chickens and their dam At one fell swoop?");
        let hits = corpus.find(&needle);
        let work = hits[0].work;
        let occurrence = hits[0].occurrence;
        assert!(occurrence.end_line > occurrence.start_line);

        let expanded = work.expand(&occurrence, 1);
        assert_eq!(expanded.start_line, occurrence.start_line);
        assert_eq!(expanded.end_line, occurrence.end_line);
    }

    #[test]
    fn expansion_at_a_speech_start_takes_the_lines_after_it() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        let hits = corpus.find(&fold("to be or not to be"));
        let work = hits[0].work;

        let expanded = work.expand(&hits[0].occurrence, 4);
        let text = work.quote_text(&expanded);
        assert!(text.starts_with("To be, or not to be"), "{text}");
        assert!(text.contains("slings and arrows"), "{text}");
        assert_eq!(expanded.end_line - expanded.start_line + 1, 4);
    }

    #[test]
    fn ignores_stage_directions_and_speaker_labels() {
        let corpus = Corpus::load(&workspace()).expect("corpus");
        // "Exeunt" appears in stage directions and headings; matches there must
        // never be offered as quotations.
        for hit in corpus.find(&fold("exeunt")) {
            let line = hit.work.lines[hit.occurrence.start_line].trim();
            assert!(!line.starts_with('['), "stage direction offered: {line}");
            assert!(!is_speaker_label(line), "speaker label offered: {line}");
        }
    }
}
