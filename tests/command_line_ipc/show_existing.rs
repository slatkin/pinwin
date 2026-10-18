//! `pinwin run --show-existing` end to end (spec "Show an existing panel",
//! "One decision for a duplicate launch", D6).
//!
//! The tests use the scratch daemon, the stand-in compositor and the helpers
//! of the parent module, so no test touches the user's display or runtime
//! directory.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::{
    DAEMON_TIMEOUT, Daemon, FirstRuns, client_output, panel_names, pinwin, temp_dir,
    wait_for_panels,
};

/// The daemon's status reply, read through the real client.
fn status(dir: &Path) -> serde_json::Value {
    let output = client_output(pinwin(dir, &["status"]));
    assert_eq!(
        output.status.code(),
        Some(0),
        "status answers: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the reply is one JSON object")
}

/// A `pinwin run --show-existing` that finds a hidden panel shows it, runs
/// nothing and exits 0; a second call leaves a shown panel unchanged (spec
/// "Show an existing panel": "Show a hidden panel", "Panel already shows").
#[test]
fn a_hidden_panel_shows_and_a_shown_panel_stays() {
    let dir = temp_dir("show-existing-hidden");
    let daemon = Daemon::start(&dir);
    let mut runs = FirstRuns::new(&dir);
    runs.start_with(&["run", "--name", "notes", "--hidden", "--", "sleep", "30"]);
    let reply = wait_for_panels(&dir, &["notes"]);
    assert_eq!(
        reply["panels"][0]["shown"],
        serde_json::json!(false),
        "the panel starts hidden: {reply}"
    );
    let pid = reply["panels"][0]["pid"].clone();

    let marker = dir.join("the-command-ran");
    let marker = marker.to_str().expect("a UTF-8 marker path");
    let output = client_output(pinwin(
        &dir,
        &[
            "run",
            "--name",
            "notes",
            "--show-existing",
            "--",
            "touch",
            marker,
        ],
    ));
    assert_eq!(
        output.status.code(),
        Some(0),
        "the existing run exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !Path::new(marker).exists(),
        "the second command did not run"
    );
    let reply = status(&dir);
    assert_eq!(
        reply["panels"].as_array().map(Vec::len),
        Some(1),
        "one panel owns the name: {reply}"
    );
    assert_eq!(
        reply["panels"][0]["shown"],
        serde_json::json!(true),
        "the hidden panel shows: {reply}"
    );
    assert_eq!(reply["panels"][0]["pid"], pid, "the same panel shows");

    // The panel already shows, so the next call changes nothing and still
    // exits 0 without running its command.
    let marker = dir.join("the-command-ran-again");
    let marker = marker.to_str().expect("a UTF-8 marker path");
    let output = client_output(pinwin(
        &dir,
        &[
            "run",
            "--name",
            "notes",
            "--show-existing",
            "--",
            "touch",
            marker,
        ],
    ));
    assert_eq!(output.status.code(), Some(0), "a shown panel exits 0");
    assert!(
        !Path::new(marker).exists(),
        "the second command did not run"
    );
    let reply = status(&dir);
    assert_eq!(
        reply["panels"][0]["shown"],
        serde_json::json!(true),
        "the shown panel stays shown: {reply}"
    );
    assert_eq!(reply["panels"][0]["pid"], pid, "the same panel stays");

    drop(runs);
    drop(daemon);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The other run options do not change the existing panel (spec "Show an
/// existing panel": "Options do not change the existing panel").
#[test]
fn show_existing_does_not_apply_the_other_run_options() {
    let dir = temp_dir("show-existing-options");
    let daemon = Daemon::start(&dir);
    let mut runs = FirstRuns::new(&dir);
    runs.start_with(&[
        "run", "--name", "notes", "--edge", "left", "--size", "400px", "--", "sleep", "30",
    ]);
    let before = wait_for_panels(&dir, &["notes"]);
    let pid = before["panels"][0]["pid"].clone();
    assert_eq!(before["panels"][0]["edge"], serde_json::json!("left"));
    assert_eq!(before["panels"][0]["size"], serde_json::json!("400px"));

    let marker = dir.join("the-command-ran");
    let marker = marker.to_str().expect("a UTF-8 marker path");
    let output = client_output(pinwin(
        &dir,
        &[
            "run",
            "--name",
            "notes",
            "--show-existing",
            "--edge",
            "right",
            "--size",
            "800px",
            "--hidden",
            "--",
            "touch",
            marker,
        ],
    ));
    assert_eq!(
        output.status.code(),
        Some(0),
        "the existing run exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !Path::new(marker).exists(),
        "the second command did not run"
    );

    let reply = status(&dir);
    let panel = &reply["panels"][0];
    assert_eq!(reply["panels"].as_array().map(Vec::len), Some(1));
    assert_eq!(panel["pid"], pid, "the same panel stays: {reply}");
    assert_eq!(panel["edge"], serde_json::json!("left"), "{reply}");
    assert_eq!(panel["size"], serde_json::json!("400px"), "{reply}");
    assert_eq!(panel["width"], serde_json::json!(400), "{reply}");
    assert_eq!(
        panel["shown"],
        serde_json::json!(true),
        "the options do not hide it: {reply}"
    );

    drop(runs);
    drop(daemon);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// With no panel of the name, `--show-existing` starts one as without the
/// flag (spec "Show an existing panel": "No panel with the name").
#[test]
fn show_existing_starts_a_panel_when_the_name_is_free() {
    let dir = temp_dir("show-existing-new");
    let daemon = Daemon::start(&dir);
    let mut runs = FirstRuns::new(&dir);
    runs.start_with(&[
        "run",
        "--name",
        "notes",
        "--show-existing",
        "--",
        "sleep",
        "30",
    ]);
    let reply = wait_for_panels(&dir, &["notes"]);
    assert_eq!(
        panel_names(&reply),
        vec!["notes".to_owned()],
        "a new panel starts: {reply}"
    );

    drop(runs);
    drop(daemon);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// Two launches at the same time give one panel; both exit 0 and only the
/// winner runs its command (spec "One decision for a duplicate launch").
#[test]
fn two_simultaneous_show_existing_launches_leave_one_panel() {
    let dir = temp_dir("show-existing-race");
    let daemon = Daemon::start(&dir);
    let marker = dir.join("launch-lines");
    let script = format!(
        "echo ran >> {}; sleep 30",
        marker.to_str().expect("a UTF-8 marker path")
    );
    let mut runs = FirstRuns::new(&dir);
    runs.start_with(&[
        "run",
        "--name",
        "notes",
        "--show-existing",
        "--",
        "sh",
        "-c",
        &script,
    ]);
    runs.start_with(&[
        "run",
        "--name",
        "notes",
        "--show-existing",
        "--",
        "sh",
        "-c",
        &script,
    ]);

    // One launch creates the panel and runs the command; the other gets the
    // existing reply and exits 0 without running it.
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    let mut exited = None;
    while exited.is_none() {
        for (index, child) in runs.children.iter_mut().enumerate() {
            if let Some(status) = child.try_wait().expect("the launch's status") {
                exited = Some((index, status));
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "one launch exits and both are still running"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let (winner, status) = exited.expect("one launch exits");
    assert_eq!(status.code(), Some(0), "the existing launch exits 0");
    assert!(
        runs.children[1 - winner]
            .try_wait()
            .expect("the winning launch's status")
            .is_none(),
        "the winning launch runs its command"
    );

    let reply = wait_for_panels(&dir, &["notes"]);
    assert_eq!(
        panel_names(&reply),
        vec!["notes".to_owned()],
        "one panel exists: {reply}"
    );
    let lines = |text: &str| text.lines().count();
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    loop {
        let count = fs::read_to_string(&marker).map_or(0, |text| lines(&text));
        if count == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the winning command ran once, not {count} times"
        );
        thread::sleep(Duration::from_millis(10));
    }

    drop(runs);
    drop(daemon);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// `--show-existing` without `--name` still refuses the invalid default name
/// and exits 2 without running the command (spec "Show an existing panel":
/// "Invalid name").
#[test]
fn show_existing_with_an_invalid_default_name_exits_2() {
    let dir = temp_dir("show-existing-invalid-name");
    let output = client_output(pinwin(&dir, &["run", "--show-existing", "--", "my.app"]));
    assert_eq!(output.status.code(), Some(2), "the name is a usage error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--name"),
        "the refusal names --name: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing runs and nothing is printed on stdout"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}
