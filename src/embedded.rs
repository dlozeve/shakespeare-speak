//! The complete works, compiled into the binary by `build.rs`.
//!
//! `WORKS` is a `&[(&str, &str)]` of `(slug, text)` pairs generated into
//! `OUT_DIR`, so the executable needs no data files at runtime.

include!(concat!(env!("OUT_DIR"), "/embedded_corpus.rs"));
