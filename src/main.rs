//! shakespeare-speak — find a verified Shakespeare quotation for a modern phrase.
//!
//! The model only ever proposes *wordings*.  A wording becomes an answer only
//! after it has been found verbatim in the local corpus, and the citation is
//! computed from that corpus, never taken from the model.  If nothing verifies,
//! the tool prints nothing and exits non-zero.

mod corpus;
mod embedded;
mod fold;
mod llm;
mod roman;
mod server;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Result};
use clap::Parser;
use serde_json::json;

use corpus::{Corpus, Kind, Placement};
use fold::fold;

/// Find a Shakespeare quotation for a modern phrase, verified against the text.
#[derive(Parser, Debug)]
#[command(name = "shakespeare-speak", version, about, long_about = None)]
struct Cli {
    /// The modern phrase, e.g. "in one go"
    #[arg(num_args = 0.., value_name = "PHRASE")]
    phrase: Vec<String>,

    /// Read the corpus from a directory instead of the copy compiled into the binary
    #[arg(long, env = "SHAKESPEARE_CORPUS", value_name = "DIR")]
    corpus: Option<PathBuf>,

    /// OpenRouter model id
    #[arg(
        long,
        env = "SHAKESPEARE_MODEL",
        default_value = "google/gemini-2.5-flash",
        value_name = "MODEL"
    )]
    model: String,

    /// OpenRouter-compatible API base URL
    #[arg(
        long,
        env = "OPENROUTER_API_BASE",
        default_value = "https://openrouter.ai/api/v1",
        value_name = "URL"
    )]
    api_base: String,

    /// How many candidate wordings to ask the model for
    #[arg(long, default_value_t = 12, value_name = "N")]
    candidates: usize,

    /// Longest quotation to return, in verses (1 keeps just the matched verse)
    #[arg(long, default_value_t = 2, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=4))]
    verses: u8,

    /// How many ranked options to show
    #[arg(long, default_value_t = 3, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=10))]
    options: u8,

    /// Skip the model and search the corpus for the phrase as typed
    #[arg(long)]
    search: bool,

    /// Skip the second pass that ranks the verified quotations
    #[arg(long)]
    no_semantic_check: bool,

    /// Show every candidate and whether it verified
    #[arg(long)]
    explain: bool,

    /// Emit JSON instead of text
    #[arg(long)]
    json: bool,

    /// Context lines to show around each quotation
    #[arg(long, default_value_t = 2, value_name = "N")]
    context: usize,

    /// Maximum hits to print in --search mode
    #[arg(long, default_value_t = 20, value_name = "N")]
    limit: usize,

    /// Run as an HTTP server instead of answering one phrase
    #[arg(long)]
    serve: bool,

    /// Address to bind when serving
    #[arg(long, default_value = "127.0.0.1:8080", value_name = "ADDR")]
    bind: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// Write once and ignore a closed pipe, so `| head` exits quietly.
fn emit(text: &str) {
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn run(cli: Cli) -> Result<ExitCode> {
    let corpus = Arc::new(load_corpus(cli.corpus.clone())?);

    let opts = Options {
        verses: cli.verses as usize,
        options: cli.options as usize,
        candidates: cli.candidates,
        context: cli.context,
        semantic_check: !cli.no_semantic_check,
    };

    if cli.serve {
        let client = match llm::api_key() {
            Ok(key) => Some(Arc::new(llm::Client::new(
                key,
                cli.model.clone(),
                cli.api_base.clone(),
            ))),
            Err(error) => {
                eprintln!("note: {error:#}");
                eprintln!("      /quote will answer 503; /search still works");
                None
            }
        };
        eprintln!(
            "corpus: {} ({} works)",
            corpus.source.as_str(),
            corpus.works.len()
        );
        server::serve(Arc::clone(&corpus), client, &cli.bind, opts)?;
        return Ok(ExitCode::SUCCESS);
    }

    let query = read_phrase(&cli.phrase)?;
    let mut out = String::new();

    if cli.search {
        let found = search(&mut out, &corpus, &query, opts.verses, cli.limit, cli.json)?;
        emit(&out);
        return Ok(if found {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
    }

    let key = llm::api_key()?;
    let client = llm::Client::new(key, cli.model.clone(), cli.api_base.clone());
    let outcome = find_answer(&corpus, &client, &query, &opts)?;

    if cli.json {
        let value = render_json(&query, &outcome, &opts);
        let _ = writeln!(out, "{}", serde_json::to_string_pretty(&value)?);
    } else {
        render_text(&mut out, &query, &outcome, cli.explain);
    }
    emit(&out);

    Ok(match outcome {
        Outcome::Found(_) => ExitCode::SUCCESS,
        Outcome::NotFound { .. } => ExitCode::from(1),
    })
}

/// Knobs shared by the CLI and the HTTP server.
#[derive(Debug, Clone)]
pub(crate) struct Options {
    pub verses: usize,
    pub options: usize,
    pub candidates: usize,
    pub context: usize,
    pub semantic_check: bool,
}

/// What the pipeline produced.
pub(crate) enum Outcome {
    Found(Found),
    NotFound { proposed: Vec<String> },
}

pub(crate) struct Found {
    /// The options to show, best first.
    pub shown: Vec<Verified>,
    /// Every verified quotation, in propose order.
    pub all: Vec<Verified>,
    /// Ranking of `all`, best first.
    pub order: Vec<usize>,
    pub proposed: Vec<String>,
    pub semantic: SemanticCheck,
    pub used_query_fallback: bool,
}

/// Run the whole propose → verify → rank pipeline.
pub(crate) fn find_answer(
    corpus: &Corpus,
    client: &llm::Client,
    query: &str,
    opts: &Options,
) -> Result<Outcome> {
    let proposed = client.propose(query, opts.candidates)?;

    let mut verified: Vec<Verified> = Vec::new();
    let mut seen: Vec<(String, usize, usize)> = Vec::new();
    for candidate in &proposed {
        if let Some(found) = verify(corpus, candidate, opts.verses, opts.context) {
            let key = (found.work_slug.clone(), found.start_line, found.end_line);
            if !seen.contains(&key) {
                seen.push(key);
                verified.push(found);
            }
        }
    }

    // If the model's wordings all missed, the phrase itself may already be
    // Shakespeare — check before giving up.
    let mut used_query_fallback = false;
    if verified.is_empty() {
        if let Some(found) = verify(corpus, query, opts.verses, opts.context) {
            used_query_fallback = true;
            verified.push(found);
        }
    }

    if verified.is_empty() {
        return Ok(Outcome::NotFound { proposed });
    }

    // Rank the survivors. The model may only reorder quotations that already
    // exist, so this cannot introduce text.
    let mut order: Vec<usize> = (0..verified.len()).collect();
    let mut semantic = SemanticCheck::Skipped;
    if verified.len() > 1 && opts.semantic_check {
        let quotes: Vec<String> = verified.iter().map(|item| item.quote.clone()).collect();
        let occurrences: Vec<usize> = verified.iter().map(|item| item.occurrences).collect();
        match client.rank(query, &quotes, &occurrences) {
            Ok(ranking) => {
                order = ranking;
                semantic = SemanticCheck::Passed;
            }
            Err(error) => {
                semantic = SemanticCheck::Unavailable;
                eprintln!("note: ranking cross-check unavailable ({error:#})");
            }
        }
    } else if verified.len() == 1 {
        semantic = SemanticCheck::SingleCandidate;
    }

    let shown: Vec<Verified> = order
        .iter()
        .take(opts.options)
        .map(|&index| verified[index].clone())
        .collect();

    Ok(Outcome::Found(Found {
        shown,
        all: verified,
        order,
        proposed,
        semantic,
        used_query_fallback,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticCheck {
    /// Only one verified candidate, so there was nothing to order.
    SingleCandidate,
    /// The model ranked the verified quotations.
    Passed,
    /// The second pass was disabled.
    Skipped,
    /// The second pass failed.
    Unavailable,
}

impl SemanticCheck {
    fn as_str(self) -> &'static str {
        match self {
            SemanticCheck::SingleCandidate => "single-candidate",
            SemanticCheck::Passed => "passed",
            SemanticCheck::Skipped => "skipped",
            SemanticCheck::Unavailable => "unavailable",
        }
    }
}

/// A quotation that has been found verbatim in the corpus.
#[derive(Debug, Clone)]
pub(crate) struct Verified {
    /// The wording the model proposed (may differ in punctuation from `quote`).
    pub candidate: String,
    /// The text exactly as it appears in the corpus, one line per verse line.
    pub quote: String,
    pub work_heading: String,
    pub work_title: String,
    pub work_slug: String,
    pub file: PathBuf,
    pub placement: Placement,
    /// 1-based line numbers.
    pub start_line: usize,
    pub end_line: usize,
    pub speaker: Option<String>,
    pub occurrences: usize,
    pub context: Vec<(usize, String)>,
    pub kind: Kind,
}

fn verify(corpus: &Corpus, candidate: &str, verses: usize, context: usize) -> Option<Verified> {
    let folded = fold(candidate);
    if folded.is_empty() {
        return None;
    }

    let hits = corpus.find(&folded);
    let occurrences = hits.len();
    let hit = hits.into_iter().next()?;

    let work = hit.work;
    let occurrence = work.expand(&hit.occurrence, verses);
    let placement = work.placement(occurrence.start_line);
    let quote = work.quote_text(&occurrence);
    if quote.is_empty() {
        return None;
    }

    let first = occurrence.start_line.saturating_sub(context);
    let last = (occurrence.end_line + context).min(work.lines.len().saturating_sub(1));
    let context_lines = (first..=last)
        .map(|index| (index + 1, work.lines[index].trim_end().to_string()))
        .collect();

    Some(Verified {
        candidate: candidate.to_string(),
        quote,
        work_heading: work.title.clone(),
        work_title: work.display_title(),
        work_slug: work.slug.clone(),
        file: work.path.clone(),
        placement,
        start_line: occurrence.start_line + 1,
        end_line: occurrence.end_line + 1,
        speaker: work.speaker(occurrence.start_line),
        occurrences,
        context: context_lines,
        kind: work.kind,
    })
}

fn citation(verified: &Verified) -> String {
    let mut parts = Vec::new();
    if let Some(sonnet) = verified.placement.sonnet {
        parts.push(format!("Sonnet {sonnet}"));
    } else if let Some(act) = verified.placement.act {
        parts.push(format!("Act {}", roman::to_lower(act).to_uppercase()));
        if let Some(scene) = verified.placement.scene {
            parts.push(format!("Scene {}", roman::to_lower(scene)));
        }
    }
    if verified.start_line == verified.end_line {
        parts.push(format!("line {}", verified.start_line));
    } else {
        parts.push(format!(
            "lines {}–{}",
            verified.start_line, verified.end_line
        ));
    }
    parts.join(" · ")
}

fn option_json(rank: usize, item: &Verified) -> serde_json::Value {
    json!({
        "rank": rank,
        "quote": item.quote,
        "work": item.work_title,
        "work_heading": item.work_heading,
        "work_slug": item.work_slug,
        "form": match item.kind {
            Kind::Play => "play",
            Kind::Sonnets => "sonnet",
            Kind::Poem => "poem",
        },
        "act": item.placement.act,
        "scene": item.placement.scene,
        "scene_roman": item.placement.scene.map(roman::to_lower),
        "sonnet": item.placement.sonnet,
        "line_start": item.start_line,
        "line_end": item.end_line,
        "file": item.file.display().to_string(),
        "speaker": item.speaker,
        "occurrences": item.occurrences,
        "context": item.context.iter()
            .map(|(number, text)| json!({ "line": number, "text": text }))
            .collect::<Vec<_>>(),
    })
}

/// The JSON body for an answer, shared by `--json` and the HTTP server.
pub(crate) fn render_json(query: &str, outcome: &Outcome, _opts: &Options) -> serde_json::Value {
    match outcome {
        Outcome::Found(found) => {
            let options: Vec<_> = found
                .shown
                .iter()
                .enumerate()
                .map(|(rank, item)| option_json(rank + 1, item))
                .collect();

            json!({
                "query": query,
                "options": options,
                "verification": {
                    "exact_in_corpus": true,
                    "citation_from_corpus": true,
                    "ranking_check": found.semantic.as_str(),
                    "candidates_proposed": found.proposed.len(),
                    "candidates_verified": found.all.len(),
                    "query_matched_directly": found.used_query_fallback,
                },
            })
        }
        Outcome::NotFound { proposed } => json!({
            "query": query,
            "options": [],
            "verified": false,
            "candidates_proposed": proposed.len(),
        }),
    }
}

fn render_text(out: &mut String, query: &str, outcome: &Outcome, explain: bool) {
    let found = match outcome {
        Outcome::Found(found) => found,
        Outcome::NotFound { proposed } => {
            let _ = writeln!(
                out,
                "No verified Shakespeare quotation found for {query:?}."
            );
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "{} candidate wording(s) were proposed; none appears verbatim in the corpus.",
                proposed.len()
            );
            if explain {
                let _ = writeln!(out);
                for candidate in proposed {
                    let _ = writeln!(out, "  ✗ {candidate}");
                }
            }
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Nothing was printed as a quotation, because nothing could be verified."
            );
            return;
        }
    };

    for (index, answer) in found.shown.iter().enumerate() {
        // Keep the verse line breaks: one output line per source line.
        let mut lines = answer.quote.lines();
        if let Some(first) = lines.next() {
            let _ = write!(out, "{}. “{first}", index + 1);
        }
        for line in lines {
            let _ = write!(out, "\n   {line}");
        }
        let _ = writeln!(out, "”");

        let _ = writeln!(out, "   {} · {}", answer.work_title, citation(answer));
        let _ = writeln!(out, "   {}", answer.file.display());
        if let Some(speaker) = &answer.speaker {
            let _ = writeln!(out, "   spoken by {speaker}");
        }

        if !answer.context.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "     context:");
            for (number, text) in &answer.context {
                let marker = if *number >= answer.start_line && *number <= answer.end_line {
                    ">"
                } else {
                    " "
                };
                let _ = writeln!(out, "     {marker} {number:>5}  {text}");
            }
        }

        if answer.occurrences > 1 {
            let _ = writeln!(
                out,
                "   · this wording occurs {} times; showing the first",
                answer.occurrences
            );
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(
        out,
        "  ✓ {} option{} verified verbatim in the corpus · citations derived from the text",
        found.shown.len(),
        if found.shown.len() == 1 { "" } else { "s" }
    );

    match found.semantic {
        SemanticCheck::Passed => {
            let _ = writeln!(
                out,
                "  ✓ meaning cross-checked by ranking the verified list"
            );
        }
        SemanticCheck::SingleCandidate => {
            let _ = writeln!(
                out,
                "  ✓ only verified candidate, so no tie-break was needed"
            );
        }
        SemanticCheck::Skipped => {
            let _ = writeln!(out, "  · ranking cross-check skipped (--no-semantic-check)");
        }
        SemanticCheck::Unavailable => {
            let _ = writeln!(
                out,
                "  ! ranking cross-check unavailable; order is the model's"
            );
        }
    }

    if found.used_query_fallback {
        let _ = writeln!(
            out,
            "  · the model proposed nothing verifiable; your phrase itself matched"
        );
    }

    if explain {
        let _ = writeln!(out);
        let _ = writeln!(out, "  audit:");
        let _ = writeln!(
            out,
            "    {} candidate wording(s) proposed by the model:",
            found.proposed.len()
        );
        for candidate in &found.proposed {
            match found
                .all
                .iter()
                .position(|item| &item.candidate == candidate)
            {
                Some(index) => {
                    let mark = match found.order.iter().position(|&i| i == index) {
                        Some(rank) if rank < found.shown.len() => {
                            format!("shown as option {}", rank + 1)
                        }
                        Some(rank) => format!("verified, ranked {}", rank + 1),
                        None => "verified".to_string(),
                    };
                    let _ = writeln!(out, "      ✓ {candidate}  [{mark}]");
                }
                None => {
                    let _ = writeln!(out, "      ✗ {candidate}  [not in corpus — discarded]");
                }
            }
        }
        if found.all.len() > 1 {
            let _ = writeln!(
                out,
                "    {} verified quotation(s); the model could only reorder them.",
                found.all.len()
            );
        }
    }
}

/// The JSON body for a direct corpus search, shared by `--search --json` and
/// the server's `/search` endpoint.
pub(crate) fn search_json(
    corpus: &Corpus,
    query: &str,
    verses: usize,
    limit: usize,
) -> serde_json::Value {
    let folded = fold(query);
    if folded.is_empty() {
        return json!({ "query": query, "total": 0, "hits": [] });
    }

    let hits = corpus.find(&folded);
    let items: Vec<_> = hits
        .iter()
        .take(limit)
        .map(|hit| {
            let occurrence = hit.work.expand(&hit.occurrence, verses);
            let placement = hit.work.placement(occurrence.start_line);
            json!({
                "quote": hit.work.quote_text(&occurrence),
                "work": hit.work.display_title(),
                "work_slug": hit.work.slug,
                "act": placement.act,
                "scene": placement.scene,
                "scene_roman": placement.scene.map(roman::to_lower),
                "sonnet": placement.sonnet,
                "line_start": occurrence.start_line + 1,
                "line_end": occurrence.end_line + 1,
                "file": hit.work.path.display().to_string(),
            })
        })
        .collect();

    json!({ "query": query, "total": hits.len(), "hits": items })
}

fn search(
    out: &mut String,
    corpus: &Corpus,
    query: &str,
    verses: usize,
    limit: usize,
    as_json: bool,
) -> Result<bool> {
    let folded = fold(query);
    if folded.is_empty() {
        bail!("nothing to search for");
    }

    let hits = corpus.find(&folded);
    if hits.is_empty() {
        if as_json {
            let _ = writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(&search_json(corpus, query, verses, limit))?
            );
        } else {
            let _ = writeln!(out, "No occurrence of {query:?} in the corpus.");
        }
        return Ok(false);
    }

    if as_json {
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&search_json(corpus, query, verses, limit))?
        );
        return Ok(true);
    }

    let _ = writeln!(out, "{} occurrence(s) of {query:?}:", hits.len());
    for hit in hits.iter().take(limit) {
        let occurrence = hit.work.expand(&hit.occurrence, verses);
        let placement = hit.work.placement(occurrence.start_line);
        let mut where_ = vec![hit.work.display_title()];
        if let Some(sonnet) = placement.sonnet {
            where_.push(format!("Sonnet {sonnet}"));
        } else if let Some(act) = placement.act {
            where_.push(format!("Act {}", roman::to_lower(act).to_uppercase()));
            if let Some(scene) = placement.scene {
                where_.push(format!("Scene {}", roman::to_lower(scene)));
            }
        }
        if occurrence.start_line == occurrence.end_line {
            where_.push(format!("line {}", occurrence.start_line + 1));
        } else {
            where_.push(format!(
                "lines {}–{}",
                occurrence.start_line + 1,
                occurrence.end_line + 1
            ));
        }
        let _ = writeln!(out);

        let text = hit.work.quote_text(&occurrence);
        let mut lines = text.lines();
        if let Some(first) = lines.next() {
            let _ = write!(out, "  “{first}");
        }
        for line in lines {
            let _ = write!(out, "\n   {line}");
        }
        let _ = writeln!(out, "”");

        let _ = writeln!(out, "    {}", where_.join(" · "));
    }
    if hits.len() > limit {
        let _ = writeln!(out);
        let _ = writeln!(out, "  … {} more (raise --limit)", hits.len() - limit);
    }
    Ok(true)
}

fn read_phrase(args: &[String]) -> Result<String> {
    let joined = args.join(" ");
    let phrase = joined.trim();
    if !phrase.is_empty() {
        return Ok(phrase.to_string());
    }

    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    let phrase = buffer.trim();
    if phrase.is_empty() {
        bail!("no phrase given; try: shakespeare-speak \"in one go\"");
    }
    Ok(phrase.to_string())
}

/// Load the corpus: from `--corpus` when given, otherwise the copy compiled in.
fn load_corpus(explicit: Option<PathBuf>) -> Result<Corpus> {
    match explicit {
        Some(dir) => {
            if !dir.is_dir() {
                bail!("corpus directory not found: {}", dir.display());
            }
            Corpus::load(&dir)
        }
        None => Corpus::embedded(),
    }
}

/// Query-string parameters, shared with the server.
pub(crate) type Params = HashMap<String, String>;
