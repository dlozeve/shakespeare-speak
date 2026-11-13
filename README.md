# shakespeare-speak

Find the Shakespeare line behind a modern phrase.

```console
$ shakespeare-speak --verses 4 "in one go"
1. “He has no children.—All my pretty ones?
   Did you say all?—O hell-kite!—All?
   What, all my pretty chickens and their dam
   At one fell swoop?”
   The Tragedy of Macbeth · Act IV · Scene iii · lines 3370–3373
   data/works/the-tragedy-of-macbeth.txt
   spoken by MACDUFF

     context:
        3368
        3369  MACDUFF.
     >  3370  He has no children.—All my pretty ones?
     >  3371  Did you say all?—O hell-kite!—All?
     >  3372  What, all my pretty chickens and their dam
     >  3373  At one fell swoop?
        3374
        3375  MALCOLM.

2. “O my good lord, the world is but a word;
   Were it all yours to give it in a breath,
   How quickly were it gone!”
   The Life of Timon of Athens · Act II · Scene ii · lines 1613–1615
   data/works/the-life-of-timon-of-athens.txt
   spoken by FLAVIUS

     context:
        1611
        1612  FLAVIUS.
     >  1613  O my good lord, the world is but a word;
     >  1614  Were it all yours to give it in a breath,
     >  1615  How quickly were it gone!
        1616
        1617  TIMON.
   · this wording occurs 2 times; showing the first

  ✓ 2 options verified verbatim in the corpus · citations derived from the text
  ✓ meaning cross-checked by ranking the verified list
```

## Why it doesn't make quotes up

A language model will invent a line and attribute it to a play without
hesitating. The tool is arranged so it can't.

The model only suggests wordings. Each suggestion is looked up in a local copy
of the complete works, and anything that isn't there is thrown away. The quote
you see is copied out of the text file. The act, scene and line numbers are
worked out by walking the `ACT` and `SCENE` headings in that same file. The
model never supplies a citation, so it can't get one wrong.

If several suggestions check out, the model gets to put them in order, but it
can only return list positions. That gives it no way to add text.

If nothing survives the lookup, the tool prints no quote and exits `1`.

What's left to the model is relevance: whether a real line actually means what
you asked. `--explain` shows what it proposed and what happened to each
suggestion, so you can judge that part yourself.

## Requirements

Rust 1.82 or newer, and an [OpenRouter](https://openrouter.ai) API key for the
model-assisted mode. `--search` needs no key.

## Build

```console
$ cargo build --release
$ ./target/release/shakespeare-speak "in one go"
```

### A single binary

The complete works are compiled into the executable. `build.rs` reads
`data/works/*.txt` and generates a table of `include_str!` entries, so the
binary needs no data files at runtime.

```console
$ cargo build --release
$ cp target/release/shakespeare-speak /tmp/ && cd /tmp
$ ./shakespeare-speak --search "at one fell swoop"
```

You do need the corpus present when you build. `--corpus <dir>` reads from a
directory instead, which is convenient while editing the texts.

For a fully static binary:

```console
$ rustup target add x86_64-unknown-linux-musl
$ cargo build --release --target x86_64-unknown-linux-musl
```

That one is about 8.8 MB and statically linked, so it runs on any x86-64 Linux,
including Alpine and distroless images. TLS comes from `rustls` rather than
OpenSSL, so there's no system certificate library to worry about.

## Usage

```console
$ export OPENROUTER_API_KEY=sk-or-...
$ shakespeare-speak "in one go"
```

The phrase can be one argument or several words:

```console
$ shakespeare-speak in one go
$ echo "in one go" | shakespeare-speak
```

### Options

| Option | Meaning |
| --- | --- |
| `--corpus <DIR>` | Read the corpus from a directory instead of the compiled-in copy |
| `--model <MODEL>` | OpenRouter model id (default `google/gemini-2.5-flash`) |
| `--api-base <URL>` | OpenRouter-compatible endpoint (default `https://openrouter.ai/api/v1`) |
| `--candidates <N>` | How many wordings to ask for (default `12`) |
| `--verses <N>` | Longest quotation, in verses, `1`–`4` (default `2`) |
| `--options <N>` | How many ranked options to show, `1`–`10` (default `3`) |
| `--search` | Skip the model; search the corpus for the phrase as typed |
| `--no-semantic-check` | Skip the second, ranking pass |
| `--explain` | Show every candidate and whether it verified |
| `--json` | Machine-readable output |
| `--context <N>` | Context lines around the quotation (default `2`) |
| `--limit <N>` | Maximum hits in `--search` mode (default `20`) |
| `--serve` | Run as an HTTP server instead of answering one phrase |
| `--bind <ADDR>` | Address to bind when serving (default `127.0.0.1:8080`) |

Environment variables: `OPENROUTER_API_KEY`, `SHAKESPEARE_MODEL`,
`SHAKESPEARE_CORPUS`, `OPENROUTER_API_BASE`.

### Quote length

`--verses` sets how many verses a quote may span.

A quote is always whole lines. Searching for `help me hence` gives you
`Help me hence, ho!` rather than the fragment the phrase happened to cover.
Line breaks are kept in both the text output and the JSON.

The window comes from the same speech, since this edition separates speeches
with blank lines and speaker labels. It ends at the match, so a line that
closes a speech brings its lead-up with it, and a line that opens one brings
what follows.

```console
$ shakespeare-speak --verses 4 --options 1 "in one go"
1. “He has no children.—All my pretty ones?
   Did you say all?—O hell-kite!—All?
   What, all my pretty chickens and their dam
   At one fell swoop?”
   The Tragedy of Macbeth · Act IV · Scene iii · lines 3370–3373

$ shakespeare-speak --verses 1 --options 1 "in one go"
1. “At one fell swoop?”
   The Tragedy of Macbeth · Act IV · Scene iii · line 3373
```

Widening never crosses into the next speech, and a match that already spans
more lines than the budget is left whole.

Prose is handled differently. Prose in this edition is wrapped mid-sentence, so
a line window would start and stop in the middle of one. When a speech looks
like prose (long lines that mostly don't end with punctuation), the quote is
aligned to sentence boundaries instead. If that would make the answer too long,
it falls back to the line window.

### Checking a result

`--explain` lists what the model proposed and what happened to each candidate.

```console
$ shakespeare-speak --explain "in one go"
...
  audit:
    4 candidate wording(s) proposed by the model:
      ✓ at one fell swoop  [chosen]
      ✗ in one go  [not in corpus — discarded]
      ✗ at a single blow  [not in corpus — discarded]
      ✗ with one fell swoop  [not in corpus — discarded]
```

### Without a model

`--search` does a folded substring search over the corpus. Handy for checking a
quotation you already have, and for testing.

```console
$ shakespeare-speak --search "cry havoc and let slip the dogs of war"
1 occurrence(s) of "cry havoc and let slip the dogs of war":

  “Cry havoc and let slip the dogs of war,”
    The Tragedy of Julius Caesar · Act III · Scene i · line 2396
```

### JSON

```console
$ shakespeare-speak --json "in one go"
```

```json
{
  "query": "in one go",
  "options": [
    {
      "rank": 1,
      "quote": "What, all my pretty chickens and their dam\nAt one fell swoop?",
      "work": "The Tragedy of Macbeth",
      "work_heading": "THE TRAGEDY OF MACBETH",
      "work_slug": "the-tragedy-of-macbeth",
      "form": "play",
      "act": 4,
      "scene": 3,
      "scene_roman": "iii",
      "sonnet": null,
      "line_start": 3372,
      "line_end": 3373,
      "file": "data/works/the-tragedy-of-macbeth.txt",
      "speaker": "MACDUFF",
      "occurrences": 1,
      "context": [{ "line": 3370, "text": "He has no children.—All my pretty ones?" }]
    }
  ],
  "verification": {
    "exact_in_corpus": true,
    "citation_from_corpus": true,
    "ranking_check": "passed",
    "candidates_proposed": 12,
    "candidates_verified": 8,
    "query_matched_directly": false
  }
}
```

Exit codes: `0` a verified quotation was printed, `1` nothing verified, `2` a
usage or configuration error.

## HTTP server

`--serve` puts the same pipeline behind HTTP. The corpus is loaded once at
startup and only `/quote` calls the model.

```console
$ shakespeare-speak --serve --bind 127.0.0.1:8080
shakespeare-speak listening on http://127.0.0.1:8080
  GET  /health
  GET  /quote?q=<phrase>[&verses=1-4][&options=1-10][&candidates=N][&context=N]
  POST /quote            {"query": "<phrase>"}
  GET  /search?q=<phrase>[&verses=1-4][&limit=N]
```

| Endpoint | Needs a key | Returns |
| --- | --- | --- |
| `GET /health` | no | `{"status":"ok","works":44,"model":…}` |
| `GET /search?q=…` | no | direct corpus hits, same shape as `--search --json` |
| `GET /quote?q=…` | yes | the answer JSON, same shape as `--json` |
| `POST /quote` | yes | the same, with `{"query": "…"}` in the body |

```console
$ curl -s "http://127.0.0.1:8080/quote?q=in+one+go&verses=4&options=1"
```

`/quote` runs the same propose, verify and rank path as the CLI, so a quotation
is only returned after being found in the corpus.

Status codes: `200` found, `404` nothing verified (the body still carries
`"verified": false`), `400` bad request, `503` no API key, `502` upstream
error.

Without `OPENROUTER_API_KEY` the server still starts and `/search` works; only
`/quote` answers `503`. Responses carry `Access-Control-Allow-Origin: *` so a
browser page can call a local instance. The default bind is loopback only; pass
`--bind 0.0.0.0:8080` if you mean to expose it.

## How matching works

The corpus is folded once at startup into a searchable form: lowercase ASCII,
accents and ligatures expanded (`æ` → `ae`), punctuation turned into word
breaks, whitespace collapsed. Candidate phrases are folded the same way, so a
hit is a real substring match rather than a fuzzy one.

Two apostrophe rules matter for this edition. Possessives and contractions lose
the apostrophe, so `'tis` folds to `tis` and `we'll` to `well`. An apostrophe
before a word-final `d` after a consonant is an elided `e` (`colour'd`,
`arm'd`, `banish'd`), so it folds to `ed`; without that, `coloured` would never
match `colour'd`.

Matches inside front matter, stage directions, speaker labels and `ACT`/`SCENE`
headings are dropped, since those aren't quotations. The exact span of a match
is recovered from the original text, so a quotation that starts mid-line is
printed from where it starts rather than as the whole wrapped line.

## The corpus

`data/works/` holds 44 files, one per work, split from the Project Gutenberg
edition (eBook #100) by `data/split.py`. Act and scene numbers come from the
`ACT`/`SCENE` headings inside each file, so a citation always points at the
line it describes.

Those files are embedded at build time. The `file` field in the JSON is the
logical source path, `data/works/the-tragedy-of-macbeth.txt`, and stays the
same whether the text came from disk or from the compiled-in copy. `/health`
reports which one is in use with `"corpus": "embedded"` or
`"corpus": "directory"`.

To rebuild the corpus from `data/complete.txt`:

```console
$ cd data && python3 split.py
$ cd .. && cargo build --release     # re-embeds the regenerated texts
```

## Tests

```console
$ cargo test
```

The suite covers the folding rules, roman numerals, JSON extraction, ranking
normalisation, verse-window expansion (including that `help me hence` comes
back as `Help me hence, ho!`) and prose sentence alignment. It also checks the
corpus itself: 38 plays with five acts each, every act's scenes numbered
contiguously from 1, sonnets numbered 1–154, no work containing another work's
front matter, and `at one fell swoop` resolving to Macbeth, Act IV, Scene iii.

The contiguity check is the one that catches gaps in the source's scene
headings. It found that Pericles Act IV writes `Scene I.` and `Scene II.` in
title case while the rest of the file uses `SCENE`, which had left those two
scenes unattributed.

## Limitations

* Relevance is the model's judgement. Existence and citation are checked
  locally every time.
* The corpus is one edition's text. A genuine Shakespeare quotation worded
  differently here won't verify, and the tool will say so rather than guess.
* Folding drops apostrophes, so a one-word query like `we'll` also matches
  `well`. Multi-word quotations aren't affected.
* `--search` is a folded substring search, not a semantic one.

## Licence

The tool is MIT. The texts in `data/` come from [Project Gutenberg](https://www.gutenberg.org/ebooks/100)
and are in the public domain.
