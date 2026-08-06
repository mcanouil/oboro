//! `oboro uninstall`, which removes everything else in this crate installs.
//!
//! The one thing every other test file must never do by accident: run the
//! shared `target/debug/oboro` binary through a confirmed, non-dry-run
//! uninstall. On Unix that binary unlinks itself, and `assert_cmd` hands out
//! the same path to every test binary in the suite, so deleting it here would
//! break whichever other test process reaches for it next. Every test that
//! confirms a real removal therefore runs a private copy of the binary
//! instead, planted inside its own workspace, where deleting it is exactly as
//! harmless as deleting any other temporary file.

mod support;

use std::path::{Path, PathBuf};

use predicates::prelude::*;
use support::Workspace;

/// A private copy of the built binary, inside `workspace`, safe to let an
/// uninstall delete.
fn private_binary(workspace: &Workspace) -> PathBuf {
    let original = assert_cmd::cargo::cargo_bin("oboro");
    let copy = workspace.path().join("oboro-under-test");
    std::fs::copy(&original, &copy).expect("copying the binary for a destructive test");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755))
            .expect("making the copy executable");
    }
    copy
}

/// Runs a just-copied private binary, retrying a handful of times on
/// `ETXTBSY`.
///
/// A binary that was written to disk a moment ago can still be seen as busy by
/// the kernel for a beat afterwards, on some filesystems more than others, and
/// that is a property of the disk under this test rather than of `oboro
/// uninstall` itself; retrying the spawn is the ordinary answer, not a
/// sleep before it that would only narrow the window.
fn run_private_binary(command: &mut std::process::Command) -> std::process::Output {
    for attempt in 0..10 {
        match command.output() {
            Ok(output) => return output,
            Err(error) if error.raw_os_error() == Some(26) && attempt < 9 => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => panic!("running a private oboro copy: {error}"),
        }
    }
    unreachable!()
}

/// An invocation of `binary`, wired to `workspace` the same way
/// [`Workspace::command`] and [`Workspace::completions_command`] are: a
/// temporary home, a vault outside it, and no ambient XDG variable left over
/// from the developer's own shell to decide where a completion script goes.
fn uninstall_command(binary: &Path, workspace: &Workspace) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command
        .current_dir(workspace.path())
        .env("HOME", workspace.home())
        .env("LC_ALL", "fr_FR.UTF-8")
        .env("LANG", "fr_FR.UTF-8")
        .env("OBORO_VAULT", workspace.path().join("vault.db"))
        .env("OBORO_KEY_FILE", workspace.path().join("key"))
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("ZSH_CUSTOM");
    command
}

/// Installs both halves in both scopes, a zsh completion, a vault outside the
/// home directory (as the harness always points it), and a model directory
/// and a stray vault under the home directory too, so a single setup exercises
/// both branches `oboro uninstall` sweeps: the wholesale directory removal
/// under `~/.oboro`, and the explicit removal of a vault `--vault` or
/// `OBORO_VAULT` points elsewhere.
fn install_everything(workspace: &Workspace) {
    for scope in ["--project", "--user"] {
        workspace
            .command()
            .arg("hook")
            .arg("install")
            .arg(scope)
            .assert()
            .success();
        workspace
            .command()
            .arg("skill")
            .arg("install")
            .arg(scope)
            .assert()
            .success();
    }
    workspace
        .completions_command()
        .arg("completions")
        .arg("zsh")
        .arg("--install")
        .assert()
        .success();

    std::fs::write(workspace.path().join("vault.db"), b"a vault").expect("planting a vault");
    std::fs::write(workspace.path().join("key"), b"a key").expect("planting a key");

    let oboro_home = workspace.home().join(".oboro");
    std::fs::create_dir_all(oboro_home.join("models/gliner-multi-pii")).expect("planting a model");
    std::fs::write(
        oboro_home.join("models/gliner-multi-pii/model.onnx"),
        b"not a real model",
    )
    .expect("planting a model file");
    // A stray vault under the default location, distinct from the one the
    // harness points OBORO_VAULT at, so the wholesale sweep of ~/.oboro is
    // what has to take it.
    std::fs::write(oboro_home.join("vault.db"), b"a default-location vault")
        .expect("planting a default-location vault");
    std::fs::write(oboro_home.join("key"), b"a default-location key")
        .expect("planting a default-location key");
}

// Unix only, for the reason given above every other `--user` test in this
// crate: `install_everything` installs into the user scope, and
// `dirs::home_dir` reads `$HOME` on Unix but asks the shell for the profile
// folder on Windows, ignoring it. On Windows this would write into, and then
// fail to find its own writes in, the runner's real profile.
#[cfg(unix)]
#[test]
fn dry_run_leaves_everything_in_place() {
    let workspace = Workspace::new();
    install_everything(&workspace);

    let output = uninstall_command(&assert_cmd::cargo::cargo_bin("oboro"), &workspace)
        .arg("uninstall")
        .arg("--dry-run")
        .output()
        .expect("running oboro uninstall");

    assert!(
        output.status.success(),
        "a dry run must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        report.contains("--dry-run: nothing was removed."),
        "{report}"
    );
    assert!(report.contains("hooks"), "{report}");
    assert!(report.contains("skill"), "{report}");
    assert!(report.contains("store"), "{report}");
    assert!(report.contains("binary"), "{report}");

    assert!(
        workspace
            .path()
            .join(".claude/settings.local.json")
            .exists()
    );
    assert!(workspace.home().join(".claude/settings.json").exists());
    assert!(
        workspace
            .path()
            .join(".claude/skills/oboro/SKILL.md")
            .exists()
    );
    assert!(
        workspace
            .home()
            .join(".claude/skills/oboro/SKILL.md")
            .exists()
    );
    assert!(workspace.home().join(".zfunc/_oboro").exists());
    assert!(workspace.path().join("vault.db").exists());
    assert!(workspace.home().join(".oboro").exists());
}

/// The confirmation this command asks for exists so that a script cannot
/// stumble into deleting a vault by piping input into the wrong command; the
/// default answer, with no terminal to answer from, has to be "do nothing".
// Unix only, for the same reason as above: `install_everything` writes to the
// user scope.
#[cfg(unix)]
#[test]
fn no_terminal_and_no_yes_refuses_and_removes_nothing() {
    let workspace = Workspace::new();
    install_everything(&workspace);

    workspace
        .command()
        .arg("uninstall")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes").or(predicate::str::contains("--dry-run")));

    assert!(
        workspace
            .path()
            .join(".claude/settings.local.json")
            .exists()
    );
    assert!(workspace.path().join("vault.db").exists());
    assert!(workspace.home().join(".oboro").exists());
}

#[test]
fn nothing_installed_is_a_clean_no_op() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("uninstall")
        .arg("--dry-run")
        .assert()
        .success()
        .stderr(predicate::str::contains("--dry-run: nothing was removed."));
}

// Unix only, for the same reason every other user-scope test in this crate
// is: `dirs::home_dir` reads `$HOME` on Unix and asks the shell for the
// profile folder on Windows, so the harness's temporary home would be
// invisible there. Binary self-removal is a Unix-only code path besides.
#[cfg(unix)]
#[test]
fn yes_removes_everything_including_the_binary() {
    let workspace = Workspace::new();
    install_everything(&workspace);
    let binary = private_binary(&workspace);

    let output = run_private_binary(
        uninstall_command(&binary, &workspace)
            .arg("uninstall")
            .arg("--yes"),
    );

    assert!(
        output.status.success(),
        "uninstalling failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The settings files belong to the user: an Oboro hook is taken out of
    // them, but the files themselves are left as `{}` rather than deleted.
    for settings in [
        workspace.path().join(".claude/settings.local.json"),
        workspace.home().join(".claude/settings.json"),
    ] {
        let text = std::fs::read_to_string(&settings).expect("the settings file must remain");
        assert!(
            !text.contains("oboro hook"),
            "{}: {text}",
            settings.display()
        );
    }
    // The `oboro` directory Oboro owns is removed; `.claude/skills` above it
    // is not, since another skill may still be using it.
    assert!(!workspace.path().join(".claude/skills/oboro").exists());
    assert!(!workspace.home().join(".claude/skills/oboro").exists());
    assert!(!workspace.home().join(".zfunc/_oboro").exists());
    assert!(!workspace.path().join("vault.db").exists());
    assert!(!workspace.path().join("key").exists());
    assert!(
        !workspace.home().join(".oboro").exists(),
        "the default-location vault, key and model must go with the directory"
    );
    assert!(!binary.exists(), "the binary must remove itself");
}

#[cfg(unix)]
#[test]
fn keep_vault_leaves_the_vault_and_key_but_still_removes_the_model() {
    let workspace = Workspace::new();
    install_everything(&workspace);
    let binary = private_binary(&workspace);

    let output = run_private_binary(
        uninstall_command(&binary, &workspace)
            .arg("uninstall")
            .arg("--yes")
            .arg("--keep-vault"),
    );

    assert!(
        output.status.success(),
        "uninstalling failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        workspace.path().join("vault.db").exists(),
        "--keep-vault must leave the vault OBORO_VAULT points at"
    );
    assert!(workspace.path().join("key").exists());
    assert!(
        workspace.home().join(".oboro/vault.db").exists(),
        "--keep-vault must leave the default-location vault too"
    );
    assert!(
        !workspace.home().join(".oboro/models").exists(),
        "the recognition model is not the vault and still goes"
    );
    let settings = std::fs::read_to_string(workspace.path().join(".claude/settings.local.json"))
        .expect("the settings file must remain");
    assert!(
        !settings.contains("oboro hook"),
        "--keep-vault only concerns the store, not the hooks: {settings}"
    );
}

/// A hook belonging to another tool on the same event is not Oboro's to
/// remove, and the settings file it lives in has to stay valid JSON once
/// Oboro's own entry is gone.
#[cfg(unix)]
#[test]
fn a_foreign_hook_survives_and_the_settings_stay_valid_json() {
    let workspace = Workspace::new();
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("creating .claude");
    std::fs::write(
        workspace.path().join(".claude/settings.local.json"),
        r#"{"hooks": {"PostToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "cargo fmt"}]}]}}"#,
    )
    .expect("planting a foreign hook");
    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--project")
        .assert()
        .success();
    let binary = private_binary(&workspace);

    let output = run_private_binary(
        uninstall_command(&binary, &workspace)
            .arg("uninstall")
            .arg("--yes"),
    );
    assert!(
        output.status.success(),
        "uninstalling failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings = std::fs::read_to_string(workspace.path().join(".claude/settings.local.json"))
        .expect("reading the settings");
    let parsed: serde_json::Value = serde_json::from_str(&settings).expect("still valid JSON");
    assert!(settings.contains("cargo fmt"), "{settings}");
    assert!(!settings.contains("oboro hook"), "{settings}");
    assert_eq!(
        parsed["hooks"]["PostToolUse"]
            .as_array()
            .expect("a list")
            .len(),
        1,
        "only Oboro's own group is removed: {settings}"
    );
}

/// A hook hand-copied into a project's shared `.claude/settings.json` is one
/// `oboro hook install` never writes, so `oboro uninstall` leaves it where it
/// is and says so, rather than rewriting a committed file or letting the user
/// believe a hook that still runs is gone.
#[cfg(unix)]
#[test]
fn a_hook_in_the_shared_settings_is_kept_and_reported() {
    let workspace = Workspace::new();
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("creating .claude");
    std::fs::write(
        workspace.path().join(".claude/settings.json"),
        r#"{"hooks": {"PostToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "oboro hook post-tool-use"}]}]}}"#,
    )
    .expect("planting a hook in the shared settings");
    let binary = private_binary(&workspace);

    let output = run_private_binary(
        uninstall_command(&binary, &workspace)
            .arg("uninstall")
            .arg("--yes"),
    );
    assert!(
        output.status.success(),
        "uninstalling failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings = std::fs::read_to_string(workspace.path().join(".claude/settings.json"))
        .expect("reading the shared settings");
    let parsed: serde_json::Value = serde_json::from_str(&settings).expect("still valid JSON");
    assert_eq!(
        parsed["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        serde_json::json!("oboro hook post-tool-use"),
        "the hook is left exactly as it was written: {settings}"
    );

    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        report.contains(".claude/settings.json") && report.contains("PostToolUse"),
        "the report names the file and the event it could not reach: {report}"
    );
}
