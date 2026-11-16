#![cfg(unix)]

//! `2brain init`: the workspace it writes is one the engine opens with nothing to
//! complain about, and the refusals that keep it from overwriting work.
//!
//! The daemon each test starts is private — its own `HOME`, so its socket,
//! registries and caches live under the temporary directory and the live vault is
//! never reachable from here.

use std::{fs, path::Path, process::Command};

use serde_json::json;

mod support;

use support::Daemon;

fn init(directory: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("init")
        .arg(directory)
        .args(extra)
        .output()
        .expect("run 2brain init")
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn init_creates_a_workspace_the_engine_opens_with_no_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let workspace = temp.path().join("my-vault");

    let created = init(&workspace, &[]);
    assert!(created.status.success(), "{}", stderr(&created));
    let stdout = String::from_utf8_lossy(&created.stdout);
    assert!(
        stdout.contains("2brain daemon start"),
        "init must name the command that comes next: {stdout}"
    );

    let identity = fs::read_to_string(workspace.join(".2brain/repo.yaml")).unwrap();
    assert!(
        identity.contains("type: au.engine.repo::au-engine"),
        "{identity}"
    );
    assert!(identity.contains("name: my-vault"), "{identity}");
    assert!(
        workspace.join("README.md").is_file(),
        "a repo self-describes"
    );
    assert!(
        workspace.join(".git").is_dir(),
        "init makes the workspace a work tree, which every write needs"
    );

    let daemon = Daemon::start(&home, &workspace);
    let answer = daemon.read(json!({"read":"overview","id":1}));
    let overview = &answer["result"]["overview"];
    assert_eq!(
        overview["members"].as_array().map(Vec::len),
        Some(1),
        "{overview}"
    );
    assert_eq!(overview["diagnostic_counts"]["total"], 0, "{overview}");
}

#[test]
fn init_refuses_a_directory_that_already_holds_files() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("occupied");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("notes.md"), "already here\n").unwrap();

    let refused = init(&workspace, &[]);
    assert!(
        !refused.status.success(),
        "a non-empty directory is refused"
    );
    let message = stderr(&refused);
    assert!(message.contains("is not empty"), "{message}");
    assert!(
        message.contains("--force"),
        "the refusal names the way past it: {message}"
    );
    assert!(
        message.contains("notes.md"),
        "the refusal names what it found: {message}"
    );
    assert!(!workspace.join(".2brain").exists(), "nothing was written");
    assert_eq!(
        fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "already here\n"
    );

    let forced = init(&workspace, &["--force"]);
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert!(workspace.join(".2brain/repo.yaml").is_file());
    assert_eq!(
        fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "already here\n",
        "--force adds a workspace, it does not clear the directory"
    );
}

#[test]
fn init_refuses_twice_and_leaves_the_first_identity_alone() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("twice");

    let first = init(&workspace, &[]);
    assert!(first.status.success(), "{}", stderr(&first));
    let identity = fs::read_to_string(workspace.join(".2brain/repo.yaml")).unwrap();

    let second = init(&workspace, &["--force"]);
    assert!(
        !second.status.success(),
        "--force does not replace a workspace"
    );
    let message = stderr(&second);
    assert!(message.contains("already a workspace"), "{message}");
    assert_eq!(
        fs::read_to_string(workspace.join(".2brain/repo.yaml")).unwrap(),
        identity,
        "a second init must not touch the identity it found"
    );
}

#[test]
fn init_copies_a_named_template_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let templates = temp.path().join("templates");
    let starter = templates.join("starter");
    fs::create_dir_all(starter.join("notes")).unwrap();
    fs::write(starter.join("Welcome.md"), "# Welcome\n").unwrap();
    fs::write(starter.join("notes/first.md"), "one\n").unwrap();
    let workspace = temp.path().join("from-template");

    let created = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("init")
        .arg(&workspace)
        .args(["--template", "starter"])
        .env("2BRAIN_TEMPLATES", &templates)
        .output()
        .expect("run 2brain init --template");
    assert!(created.status.success(), "{}", stderr(&created));
    assert_eq!(
        fs::read_to_string(workspace.join("notes/first.md")).unwrap(),
        "one\n"
    );
    assert!(workspace.join("Welcome.md").is_file());
    assert!(workspace.join(".2brain/repo.yaml").is_file());
}

#[test]
fn init_names_every_place_it_looked_for_a_missing_template() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("no-such-template");

    let refused = init(&workspace, &["--template", "absent"]);
    assert!(!refused.status.success());
    let message = stderr(&refused);
    assert!(message.contains("no template named"), "{message}");
    assert!(message.contains("tb-defaults"), "{message}");
    assert!(
        !workspace.join(".2brain").exists(),
        "a missing template leaves no half-written workspace"
    );
}

#[test]
fn init_adopts_a_template_identity_and_renames_only_it() {
    let temp = tempfile::tempdir().unwrap();
    let templates = temp.path().join("templates");
    let starter = templates.join("adopted");
    fs::create_dir_all(starter.join(".2brain")).unwrap();
    fs::write(
        starter.join(".2brain/repo.yaml"),
        "# a starter that carries its own identity\ntype: au.engine.repo::au-engine\nname: adopted\ndeps:\n  - name: base\n",
    )
    .unwrap();
    fs::write(
        starter.join(".2brain/workspace.yaml"),
        "type: au.engine.workspace::au-engine\nedit:\n  - adopted   # the containing repo, rewritten on create\ndiscover:\n  - bundle\n",
    )
    .unwrap();
    let workspace = temp.path().join("renamed");

    let created = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("init")
        .arg(&workspace)
        .args(["--template", "adopted"])
        .env("2BRAIN_TEMPLATES", &templates)
        .output()
        .expect("run 2brain init --template");
    assert!(created.status.success(), "{}", stderr(&created));

    let identity = fs::read_to_string(workspace.join(".2brain/repo.yaml")).unwrap();
    assert!(identity.contains("name: renamed"), "{identity}");
    assert!(
        identity.contains("deps:\n  - name: base"),
        "a starter's own deps must survive the rename: {identity}"
    );
    assert!(
        identity.contains("type: au.engine.repo::au-engine"),
        "{identity}"
    );
    let manifest = fs::read_to_string(workspace.join(".2brain/workspace.yaml")).unwrap();
    assert!(manifest.contains("- renamed"), "{manifest}");
    assert!(
        manifest.contains("the containing repo, rewritten on create"),
        "a rename keeps the rest of the line: {manifest}"
    );
    assert!(manifest.contains("- bundle"), "{manifest}");
}

/// The project-memory starter is the one template that ships TYPES, so its value is
/// that a fresh workspace validates and answers its own questions on the first run.
/// Nothing here checks ranking; it checks that `2brain init --template project-memory`
/// produces a workspace the engine accepts.
#[test]
fn the_project_memory_template_ships_the_six_record_types() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let workspace = temp.path().join("memory");
    let done = init(&workspace, &["--template", "project-memory"]);
    assert!(
        done.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&done.stderr)
    );

    let types: Vec<String> = std::fs::read_dir(workspace.join("type"))
        .expect("the template ships a type directory")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    for name in [
        "decision",
        "constraint",
        "evidence",
        "experiment",
        "handoff",
        "error-fix",
    ] {
        assert!(
            types.iter().any(|file| file == &format!("{name}.type.yaml")),
            "{name}.type.yaml is missing from {types:?}"
        );
    }

    // The engine must accept the workspace as it ships, with no diagnostics: a starter whose
    // own types error is worse than no starter.
    let daemon = Daemon::start(&home, &workspace);
    let answer = daemon.read(json!({"read":"overview","id":1}));
    let overview = &answer["result"]["overview"];
    assert_eq!(
        overview["diagnostic_counts"]["total"], 0,
        "the shipped template must validate: {overview}"
    );
    let handoff = daemon.read(json!({"read":"instances_of","id":1,"type":"handoff"}));
    assert!(
        handoff["result"]["instances_of"].is_array(),
        "the template's own types resolve: {handoff}"
    );

    // The identity the template carries is adopted, not copied verbatim: the engine
    // refuses a repo whose folder name and `name:` disagree, and the next agent reads
    // the name in every path it is given.
    let repo = std::fs::read_to_string(workspace.join(".2brain/repo.yaml")).unwrap();
    assert!(repo.contains("name: memory\n"), "{repo}");
    assert!(!repo.contains("name: project-memory\n"), "{repo}");

    // Every required field in every type is one the template's own guide explains;
    // a type whose required field is a URL would make the honest record the rare one.
    let decision = std::fs::read_to_string(workspace.join("type/decision.type.yaml")).unwrap();
    assert!(decision.contains("description: String"), "{decision}");
    assert!(
        !decision.contains("\n  source_url: String"),
        "source_url must stay optional: {decision}"
    );
}
