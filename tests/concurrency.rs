//! Several `oboro` processes sharing one vault.
//!
//! An agent hook cleans whatever the tool it wraps produced, so nothing keeps
//! two invocations from reaching the vault at the same instant. Allocation has
//! to survive that: every process must succeed, and a value must map to one
//! placeholder however many processes saw it first.

mod support;

use std::io::Write as _;

use support::Workspace;

/// Processes racing over one document.
const RACERS: usize = 8;

/// Values in that document.
///
/// Enough of them that a process spends long enough allocating for the others
/// to catch up: one value each, and process startup alone would keep the
/// racers apart.
const VALUES: usize = 150;

/// A distinct French mobile number for `index`, in the compact spelling the
/// detector reads as readily as the spaced one.
fn value(index: usize) -> String {
    format!("0{}", 612_000_000 + index * 137)
}

/// The document every racer cleans.
fn document() -> String {
    use std::fmt::Write as _;

    (0..VALUES).fold(String::new(), |mut text, index| {
        writeln!(text, "Call {}.", value(index)).expect("writing to a string");
        text
    })
}

#[test]
fn concurrent_cleans_agree_on_every_placeholder() {
    let workspace = Workspace::new();

    // The vault, its key and its schema are created first, so the race is over
    // allocation alone rather than over first-use setup.
    workspace
        .command()
        .arg("map")
        .arg("list")
        .assert()
        .success();

    let text = document();

    // Standard input is the starting gun: `clean -` reads it to the end before
    // it opens the vault, so every process is already running when its pipe
    // closes. Writing as each one is spawned would release them one by one and
    // there would be no race to lose.
    let mut children = Vec::with_capacity(RACERS);
    let mut pipes = Vec::with_capacity(RACERS);
    for _ in 0..RACERS {
        let mut command = workspace.std_command("fr_FR.UTF-8");
        let mut child = command
            .arg("clean")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawning oboro clean");
        pipes.push(child.stdin.take().expect("the child's standard input"));
        children.push(child);
    }
    for mut pipe in pipes {
        pipe.write_all(text.as_bytes())
            .expect("writing to the child");
    }

    let outputs: Vec<_> = children
        .into_iter()
        .map(|child| child.wait_with_output().expect("waiting for oboro clean"))
        .collect();

    for output in &outputs {
        assert!(
            output.status.success(),
            "a racing process failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Which process allocated a given value does not matter: allocation is
    // recorded in the vault they share, so all of them must report the same
    // document.
    let cleaned: Vec<String> = outputs
        .iter()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .collect();
    for document in &cleaned {
        assert_eq!(
            document, &cleaned[0],
            "racing processes disagreed on the placeholders for one document"
        );
    }

    // One allocation per value, not one per process that thought it was first.
    let listing = workspace
        .command()
        .arg("map")
        .arg("list")
        .output()
        .expect("running oboro map list");
    let entries = String::from_utf8_lossy(&listing.stdout);
    assert_eq!(
        entries
            .lines()
            .filter(|line| line.contains("PHONE"))
            .count(),
        VALUES,
        "the vault must hold one mapping per value, got:\n{entries}"
    );
}
