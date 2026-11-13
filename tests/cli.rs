//! End-to-end checks of the binary in `--search` mode, which needs no API key.

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_shakespeare-speak"))
        .args(args)
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("SHAKESPEARE_CORPUS")
        .output()
        .expect("run shakespeare-speak")
}

#[test]
fn finds_a_quote_with_act_and_scene() {
    let output = run(&["--search", "at one fell swoop"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("At one fell swoop?"), "{stdout}");
    assert!(stdout.contains("The Tragedy of Macbeth"), "{stdout}");
    assert!(stdout.contains("Act IV"), "{stdout}");
    assert!(stdout.contains("Scene iii"), "{stdout}");
}

#[test]
fn attributes_a_sonnet() {
    let output = run(&["--search", "shall i compare thee to a summers day"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Sonnet 18"), "{stdout}");
    assert!(
        stdout.contains("Shall I compare thee to a summer’s day?"),
        "{stdout}"
    );
}

#[test]
fn reports_absence_with_a_nonzero_exit() {
    let output = run(&["--search", "this phrase is definitely not in shakespeare"]);
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No occurrence"), "{stdout}");
}

#[test]
fn emits_machine_readable_json() {
    let output = run(&["--json", "--search", "--verses", "1", "at one fell swoop"]);
    assert!(output.status.success());

    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON on stdout");
    let hit = &value["hits"][0];

    assert_eq!(hit["work_slug"], "the-tragedy-of-macbeth");
    assert_eq!(hit["act"], 4);
    assert_eq!(hit["scene"], 3);
    assert_eq!(hit["scene_roman"], "iii");
    assert_eq!(hit["quote"], "At one fell swoop?");
    assert_eq!(hit["line_start"], 3373);
    assert_eq!(hit["line_end"], 3373);
}

#[test]
fn returns_the_whole_verse_not_a_slice_of_it() {
    // The phrase stops at "hence," but the verse is "Help me hence, ho!".
    let output = run(&["--search", "help me hence"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Help me hence, ho!"), "{stdout}");
}

#[test]
fn preserves_verse_line_breaks() {
    let output = run(&["--json", "--search", "--verses", "2", "at one fell swoop"]);
    assert!(output.status.success());

    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON on stdout");
    let hit = &value["hits"][0];

    assert_eq!(
        hit["quote"],
        "What, all my pretty chickens and their dam\nAt one fell swoop?"
    );
    assert_eq!(hit["line_start"], 3372);
    assert_eq!(hit["line_end"], 3373);
}

#[test]
fn works_without_a_data_directory() {
    // The corpus is compiled into the binary, so running from a directory that
    // has no data/ must still work.
    let output = Command::new(env!("CARGO_BIN_EXE_shakespeare-speak"))
        .args(["--search", "at one fell swoop"])
        .current_dir(std::env::temp_dir())
        .env_remove("SHAKESPEARE_CORPUS")
        .env_remove("OPENROUTER_API_KEY")
        .output()
        .expect("run shakespeare-speak");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("At one fell swoop?"), "{stdout}");
    assert!(stdout.contains("The Tragedy of Macbeth"), "{stdout}");
}

#[test]
fn rejects_an_empty_phrase() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn rejects_a_missing_corpus() {
    let output = run(&["--search", "--corpus", "/nonexistent-corpus", "x"]);
    assert_eq!(output.status.code(), Some(2));
}
