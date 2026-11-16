#![cfg(unix)]

//! Two adapter PROCESSES creating one path at the same instant, against one
//! temporary engine.
//!
//! `tb_create_file` used to read absence and then send a plain `write_file`: two
//! requests, so two clients could both read absence and the second silently
//! overwrote the first. The engine's `create_file` makes the absence check part
//! of the write, and this test is the arrangement that defect needed — one
//! mutation slot per PROCESS, so the slot cannot serialise the pair — asserting
//! one success, one refusal, and the winner's bytes on disk.
//!
//! The engine is private: its own `HOME`, so the socket, the registries and the
//! caches live under the temporary directory and nothing here can reach the
//! live vault.

use std::{fs, path::Path, process::Command};

use serde_json::json;

mod support;

use support::{Client, Daemon, error_text};

/// Enough rounds that a pass is the absence of the interleaving, not luck.
const ROUNDS: usize = 20;

/// The workspace `2brain init` writes, plus the seed commit the engine's
/// clean-at-HEAD precondition needs.
fn seeded_workspace(home: &Path, root: &Path) -> String {
    fs::create_dir_all(home).unwrap();
    let created = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("init")
        .arg(root)
        .env("HOME", home)
        .output()
        .expect("run 2brain init");
    assert!(
        created.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=2brain-race",
                "-c",
                "user.email=race@example.invalid",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} in {}", root.display());
    };
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "seed"]);
    root.file_name()
        .and_then(|name| name.to_str())
        .expect("the workspace has a name")
        .to_owned()
}

#[test]
fn two_processes_creating_one_path_produce_one_success_and_one_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("race-ws");
    let member = seeded_workspace(&home, &root);
    let daemon = Daemon::start(&home, &root);

    // Two client PROCESSES, each with its own mutation slot and its own read of
    // the path: the pair the engine-side check exists for.
    let mut left = Client::start_with(daemon.socket(), &["--allow-mutations"]);
    let mut right = Client::start_with(daemon.socket(), &["--allow-mutations"]);

    for round in 0..ROUNDS {
        let path = format!("races/round-{round}.md");
        let left_content = format!("left {round}\n");
        let right_content = format!("right {round}\n");
        // Both calls go out before either reply is read, so the processes race
        // rather than alternate.
        left.send_call(
            2,
            "tb_create_file",
            json!({"repo":&member,"path":&path,"content":&left_content}),
        );
        right.send_call(
            2,
            "tb_create_file",
            json!({"repo":&member,"path":&path,"content":&right_content}),
        );
        let answered_left = left.receive();
        let answered_right = right.receive();

        let left_won = answered_left["result"]["isError"] != true;
        let right_won = answered_right["result"]["isError"] != true;
        assert!(
            left_won ^ right_won,
            "round {round}: exactly one create may succeed, got {answered_left} and {answered_right}"
        );

        let refusal = if left_won {
            error_text(&answered_right)
        } else {
            error_text(&answered_left)
        };
        assert_eq!(
            refusal, "policy/absence: file already exists; use tb_write_file to change it",
            "round {round}: the loser hears the absence gate, not an engine message"
        );

        // The winner's bytes, whole: no interleaved write, no truncation.
        let winner = if left_won {
            &left_content
        } else {
            &right_content
        };
        let on_disk = fs::read_to_string(root.join(&path)).unwrap();
        assert_eq!(
            &on_disk, winner,
            "round {round}: the file holds the winner's bytes"
        );
    }
}
