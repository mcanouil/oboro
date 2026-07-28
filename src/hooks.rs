//! Naming the agent hooks in the user's settings, and finding out whether they
//! are there.
//!
//! A user can believe they are protected while nothing is wired up, so
//! `oboro doctor` reports what it can see and protection is something to verify
//! rather than assume.
//!
//! Writing them is the harder of Oboro's two installs. The settings file
//! belongs to the user and holds hooks Oboro knows nothing about, unlike the
//! skill in `crate::skill`, which is a file Oboro creates and owns outright. So
//! the merge only ever adds: every other key keeps its place and its order, an
//! Oboro hook that is already there is left exactly as it was written, and a
//! file that cannot be parsed is refused rather than replaced.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::claude::{Scope, refuse_symlinks, write_atomic};

/// An event Oboro answers.
pub struct Event {
    /// The event as the agent names it, such as `PostToolUse`.
    pub name: &'static str,
    /// The `oboro hook` subcommand that answers it.
    pub subcommand: &'static str,
    /// The tools it is worth running against, written into the settings when
    /// Oboro installs the hook.
    pub matcher: &'static str,
}

/// The two events Oboro answers.
///
/// The matchers are the ones the documentation has always shown: the tools that
/// bring outside text to the model, and the tools that put the model's text on
/// disk.
pub const EVENTS: [Event; 2] = [
    Event {
        name: "PostToolUse",
        subcommand: "post-tool-use",
        matcher: "Read|Grep|Bash|WebFetch",
    },
    Event {
        name: "PreToolUse",
        subcommand: "pre-tool-use",
        matcher: "Write|Edit",
    },
];

/// A hook Oboro found in a settings file.
pub struct Installed {
    /// The event it answers, such as `PostToolUse`.
    pub event: &'static str,
    /// The settings file naming it.
    pub file: PathBuf,
    /// The tools it is matched against, as written in the settings.
    pub matcher: Option<String>,
    /// The command as written, so a user can see what will run.
    pub command: String,
}

/// The settings files an agent reads, nearest first.
///
/// Project settings come before user settings because that is the order a
/// reader would check them in, not because one wins: a hook in either is a hook
/// that runs.
fn settings_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = vec![
        cwd.join(".claude/settings.json"),
        cwd.join(".claude/settings.local.json"),
    ];
    if let Some(home) = dirs::home_dir() {
        files.push(home.join(".claude/settings.json"));
    }
    files
}

/// Every Oboro hook named in the settings files reachable from `cwd`.
///
/// A file that does not exist, cannot be read, or holds invalid JSON
/// contributes nothing: this is a report, and a settings file Oboro cannot
/// parse is the agent's business rather than something to fail over.
#[must_use]
pub fn installed_from(cwd: &Path) -> Vec<Installed> {
    let mut found = Vec::new();
    for file in settings_files(cwd) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        for event in EVENTS {
            for (command, matcher) in commands_for(&settings, event.name) {
                if names_oboro_hook(&command, event.subcommand) {
                    found.push(Installed {
                        event: event.name,
                        file: file.clone(),
                        matcher,
                        command,
                    });
                }
            }
        }
    }
    found
}

/// Every hook command configured for `event`, paired with its matcher.
fn commands_for(settings: &serde_json::Value, event: &str) -> Vec<(String, Option<String>)> {
    let Some(groups) = settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };

    let mut commands = Vec::new();
    for group in groups {
        let matcher = group
            .get("matcher")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let entries = group
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for entry in entries {
            if let Some(command) = entry.get("command").and_then(serde_json::Value::as_str) {
                commands.push((command.to_owned(), matcher.clone()));
            }
        }
    }
    commands
}

/// Whether `command` runs `oboro hook <subcommand>`.
///
/// Matched on the two words rather than the whole string, so a wrapper script,
/// an absolute path or a `cargo run` during development are all recognised.
fn names_oboro_hook(command: &str, subcommand: &str) -> bool {
    command.contains("oboro") && command.contains("hook") && command.contains(subcommand)
}

/// Where a scope's hooks are written, below its root.
///
/// A project install goes to `settings.local.json` rather than `settings.json`:
/// turning on a hook that intercepts every `Read`, `Grep`, `Bash` and
/// `WebFetch` is a decision for whoever is sitting at the machine, and a
/// committed hook naming a binary a colleague has not installed fails closed on
/// every matching tool call. `settings_files` reads both, so `doctor` reports
/// either.
///
/// # Errors
///
/// Returns an error for the same reason [`Scope::root`] does.
pub fn settings_path(scope: Scope, cwd: &Path) -> Result<PathBuf> {
    Ok(scope
        .root(cwd)?
        .join(settings_components(scope).iter().collect::<PathBuf>()))
}

/// The components of a scope's settings path, one directory each.
///
/// Split rather than spelled as one `.claude/settings.json`, because
/// [`refuse_symlinks`] walks these and a component carrying a trailing slash
/// makes the check follow the link it is meant to catch: `symlink_metadata`
/// resolves `.claude/` where it reports on `.claude`.
fn settings_components(scope: Scope) -> [&'static str; 2] {
    match scope {
        Scope::Project => [".claude", "settings.local.json"],
        Scope::User => [".claude", "settings.json"],
    }
}

/// What installing into a settings file would do to one event.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// The hook is not there, and this matcher will be written.
    Add(&'static str),
    /// An Oboro hook is already there, matched against this, and is left as it
    /// was written. `None` is a hook with no matcher, meaning every tool.
    Keep(Option<String>),
}

/// What installing into a scope would do, decided once so that what is
/// announced and what is written cannot disagree.
#[derive(Debug)]
pub struct Plan {
    /// The settings file this would write.
    pub file: PathBuf,
    /// What happens to each event, in the order [`EVENTS`] lists them.
    pub changes: Vec<(&'static str, Change)>,
    /// The settings as they would end up, so `--dry-run` can show them.
    settings: serde_json::Value,
}

impl Plan {
    /// Whether anything would actually be written.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.changes
            .iter()
            .any(|(_, change)| matches!(change, Change::Add(_)))
    }

    /// The settings as they would end up, formatted the way they would land.
    ///
    /// # Errors
    ///
    /// Returns an error if the merged settings cannot be rendered, which a
    /// value parsed from JSON should never fail to be. Reporting it beats any
    /// placeholder string, since this is what gets written: a stand-in for the
    /// settings would be a settings file that is not JSON.
    pub fn rendered(&self) -> Result<String> {
        let text = serde_json::to_string_pretty(&self.settings)
            .with_context(|| format!("rendering the settings for {}", self.file.display()))?;
        Ok(format!("{text}\n"))
    }
}

/// What installing Oboro's hooks into `scope` would do, without doing it.
///
/// # Errors
///
/// Returns an error when the scope has no root, when the path to the settings
/// file passes through a symbolic link, or when the file exists but is not JSON
/// this can merge into. A file Oboro cannot read is one it must not replace.
pub fn plan(scope: Scope, cwd: &Path) -> Result<Plan> {
    let root = scope.root(cwd)?;
    refuse_symlinks(&root, &settings_components(scope))?;
    let file = settings_path(scope, cwd)?;

    let mut settings = read_settings(&file)?;
    let mut changes = Vec::new();
    for event in EVENTS {
        changes.push((event.name, merge(&mut settings, &event, &file)?));
    }
    Ok(Plan {
        file,
        changes,
        settings,
    })
}

/// Carries out a [`Plan`], returning it so the caller can report what happened.
///
/// # Errors
///
/// Returns an error when the directory or the settings file cannot be written.
pub fn install(plan: Plan) -> Result<Plan> {
    if !plan.writes() {
        return Ok(plan);
    }
    if let Some(parent) = plan.file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    write_atomic(&plan.file, plan.rendered()?.as_bytes())?;
    Ok(plan)
}

/// The settings as they stand, or an empty object when there is no file yet.
fn read_settings(file: &Path) -> Result<serde_json::Value> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::json!({}));
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", file.display())),
    };
    // An empty file is a settings file someone created and never filled in,
    // which is an empty object rather than something to refuse over.
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }

    let settings: serde_json::Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "parsing {}, which Oboro will not replace unread",
            file.display()
        )
    })?;
    if !settings.is_object() {
        bail!(
            "{} holds {} rather than an object, so there is nowhere to name a hook",
            file.display(),
            kind_of(&settings)
        );
    }
    Ok(settings)
}

/// Adds `event`'s hook to `settings` unless an Oboro hook is already there.
fn merge(settings: &mut serde_json::Value, event: &Event, file: &Path) -> Result<Change> {
    if let Some(kept) = already_named(settings, event) {
        return Ok(kept);
    }

    let hooks = settings
        .as_object_mut()
        .expect("the root was checked to be an object")
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        bail!(
            "`hooks` in {} holds {} rather than an object",
            file.display(),
            kind_of(hooks)
        );
    }
    let groups = hooks
        .as_object_mut()
        .expect("just checked")
        .entry(event.name)
        .or_insert_with(|| serde_json::json!([]));
    let Some(groups) = groups.as_array_mut() else {
        bail!(
            "`hooks.{}` in {} holds {} rather than a list",
            event.name,
            file.display(),
            kind_of(groups)
        );
    };

    groups.push(serde_json::json!({
        "matcher": event.matcher,
        "hooks": [{"type": "command", "command": format!("oboro hook {}", event.subcommand)}],
    }));
    Ok(Change::Add(event.matcher))
}

/// The [`Change::Keep`] for an Oboro hook already named for `event`, if one is.
///
/// An entry naming the command is left exactly as written, matcher included: a
/// matcher someone narrowed by hand is a decision, not drift.
fn already_named(settings: &serde_json::Value, event: &Event) -> Option<Change> {
    commands_for(settings, event.name)
        .into_iter()
        .find(|(command, _)| names_oboro_hook(command, event.subcommand))
        .map(|(_, matcher)| Change::Keep(matcher))
}

/// Names a JSON value's shape, for a message about the wrong one.
fn kind_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Whether the program a hook command starts with can be run.
///
/// Only the first word is checked: everything after it is the subcommand and
/// its flags. A command given as a path is checked as a path, and a bare name
/// is looked for along `PATH`, which is how the agent will resolve it too.
#[must_use]
pub fn program_is_reachable(command: &str) -> bool {
    let Some(program) = command.split_whitespace().next() else {
        return false;
    };
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = r#"{
        "hooks": {
            "PostToolUse": [
                {
                    "matcher": "Read|Grep",
                    "hooks": [
                        {"type": "command", "command": "oboro hook post-tool-use"},
                        {"type": "command", "command": "something-else"}
                    ]
                }
            ],
            "PreToolUse": [
                {"hooks": [{"type": "command", "command": "/opt/bin/oboro hook pre-tool-use"}]}
            ]
        }
    }"#;

    #[test]
    fn a_hook_is_found_with_its_matcher() {
        let settings: serde_json::Value = serde_json::from_str(SETTINGS).expect("valid JSON");
        let commands = commands_for(&settings, "PostToolUse");

        assert_eq!(commands.len(), 2, "every command in the group is reported");
        assert_eq!(commands[0].1.as_deref(), Some("Read|Grep"));
        assert!(names_oboro_hook(&commands[0].0, "post-tool-use"));
        assert!(
            !names_oboro_hook(&commands[1].0, "post-tool-use"),
            "another tool's hook is not ours"
        );
    }

    /// A matcher is optional in the settings, and its absence means every tool,
    /// so it must not be mistaken for a missing hook.
    #[test]
    fn a_hook_without_a_matcher_is_still_a_hook() {
        let settings: serde_json::Value = serde_json::from_str(SETTINGS).expect("valid JSON");
        let commands = commands_for(&settings, "PreToolUse");

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].1, None);
        assert!(names_oboro_hook(&commands[0].0, "pre-tool-use"));
    }

    #[test]
    fn settings_without_hooks_report_nothing() {
        let settings: serde_json::Value =
            serde_json::from_str(r#"{"model": "opus", "hooks": {}}"#).expect("valid JSON");
        assert!(commands_for(&settings, "PostToolUse").is_empty());
    }

    /// The post hook must not be reported as the pre hook, or a user missing
    /// half the round trip would be told they are covered.
    #[test]
    fn one_event_is_not_mistaken_for_the_other() {
        assert!(!names_oboro_hook(
            "oboro hook post-tool-use",
            "pre-tool-use"
        ));
    }

    /// Installs into a temporary project, the way the command does.
    fn install_into(cwd: &Path) -> Result<Plan> {
        install(plan(Scope::Project, cwd)?)
    }

    fn settings_in(cwd: &Path) -> serde_json::Value {
        let file = settings_path(Scope::Project, cwd).expect("a path");
        serde_json::from_str(&std::fs::read_to_string(file).expect("reading the settings"))
            .expect("valid JSON")
    }

    /// Every Oboro command named in the settings just written, read back from
    /// the file.
    ///
    /// Deliberately not `installed_from`, which also reads `~/.claude` and
    /// would count the hooks a developer has installed on their own machine.
    fn commands_written(cwd: &Path) -> Vec<String> {
        let settings = settings_in(cwd);
        EVENTS
            .iter()
            .flat_map(|event| {
                commands_for(&settings, event.name)
                    .into_iter()
                    .map(|(command, _)| command)
                    .filter(|command| names_oboro_hook(command, event.subcommand))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn write_settings(cwd: &Path, text: &str) {
        let file = settings_path(Scope::Project, cwd).expect("a path");
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("creating .claude");
        std::fs::write(file, text).expect("writing the settings");
    }

    #[test]
    fn a_project_with_no_settings_gets_both_hooks() {
        let project = tempfile::tempdir().expect("temporary directory");

        let done = install_into(project.path()).expect("installing");

        assert_eq!(
            done.changes,
            vec![
                ("PostToolUse", Change::Add("Read|Grep|Bash|WebFetch")),
                ("PreToolUse", Change::Add("Write|Edit")),
            ]
        );
        assert_eq!(
            commands_written(project.path()),
            vec![
                "oboro hook post-tool-use".to_owned(),
                "oboro hook pre-tool-use".to_owned(),
            ],
            "the commands written must be the ones doctor looks for"
        );
    }

    /// The project install must not touch `settings.json`, which is the file a
    /// team shares, and must not decide for anyone but the person running it.
    #[test]
    fn a_project_install_writes_only_the_local_settings() {
        let project = tempfile::tempdir().expect("temporary directory");

        install_into(project.path()).expect("installing");

        assert!(project.path().join(".claude/settings.local.json").exists());
        assert!(!project.path().join(".claude/settings.json").exists());
    }

    /// The settings file belongs to the user. Rewriting the keys they wrote,
    /// even into a valid file, is a change they did not ask for.
    #[test]
    fn every_other_key_keeps_its_place_and_its_order() {
        let project = tempfile::tempdir().expect("temporary directory");
        write_settings(
            project.path(),
            r#"{"zebra": 1, "permissions": {"allow": ["Bash(ls)"]}, "alpha": 2}"#,
        );

        install_into(project.path()).expect("installing");

        let text =
            std::fs::read_to_string(settings_path(Scope::Project, project.path()).expect("a path"))
                .expect("reading the settings");
        let keys: Vec<_> = serde_json::from_str::<serde_json::Value>(&text)
            .expect("valid JSON")
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, ["zebra", "permissions", "alpha", "hooks"]);
        assert!(text.contains("Bash(ls)"), "other settings must survive");
    }

    /// Someone else's hook on the same event is not Oboro's to move or drop.
    #[test]
    fn another_tools_hook_on_the_same_event_is_kept() {
        let project = tempfile::tempdir().expect("temporary directory");
        write_settings(
            project.path(),
            r#"{"hooks": {"PostToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "cargo fmt"}]}]}}"#,
        );

        install_into(project.path()).expect("installing");

        let groups = settings_in(project.path())["hooks"]["PostToolUse"]
            .as_array()
            .expect("a list")
            .clone();
        assert_eq!(groups.len(), 2, "ours is appended, not substituted");
        assert_eq!(groups[0]["hooks"][0]["command"], "cargo fmt");
        assert_eq!(groups[1]["matcher"], "Read|Grep|Bash|WebFetch");
    }

    #[test]
    fn installing_twice_changes_nothing_the_second_time() {
        let project = tempfile::tempdir().expect("temporary directory");
        install_into(project.path()).expect("installing");
        let after_first = settings_in(project.path());

        let done = install_into(project.path()).expect("installing again");

        assert!(!done.writes());
        assert_eq!(settings_in(project.path()), after_first);
    }

    /// A matcher someone narrowed by hand is a decision. Widening it back would
    /// be Oboro overruling them on their own machine.
    #[test]
    fn a_narrowed_matcher_is_left_alone_and_reported() {
        let project = tempfile::tempdir().expect("temporary directory");
        write_settings(
            project.path(),
            r#"{"hooks": {"PostToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "oboro hook post-tool-use"}]}]}}"#,
        );

        let done = install_into(project.path()).expect("installing");

        assert_eq!(
            done.changes,
            vec![
                ("PostToolUse", Change::Keep(Some("Read".to_owned()))),
                ("PreToolUse", Change::Add("Write|Edit")),
            ],
            "the narrowed half is kept and the missing half still installed"
        );
        assert_eq!(
            settings_in(project.path())["hooks"]["PostToolUse"][0]["matcher"],
            "Read"
        );
    }

    /// `installed_from` ignores a file it cannot parse because it is only
    /// reporting. An installer that did the same would replace it.
    #[test]
    fn settings_that_are_not_json_are_refused_and_left_alone() {
        let project = tempfile::tempdir().expect("temporary directory");
        write_settings(project.path(), "{ this is not json");

        let error = install_into(project.path()).expect_err("must refuse");

        assert!(format!("{error:#}").contains("settings.local.json"));
        assert_eq!(
            std::fs::read_to_string(settings_path(Scope::Project, project.path()).expect("a path"))
                .expect("reading the settings"),
            "{ this is not json"
        );
    }

    #[test]
    fn settings_shaped_wrongly_are_refused() {
        for (text, expected) in [
            ("[1, 2, 3]", "a list"),
            (r#"{"hooks": []}"#, "a list"),
            (r#"{"hooks": {"PostToolUse": {}}}"#, "an object"),
        ] {
            let project = tempfile::tempdir().expect("temporary directory");
            write_settings(project.path(), text);

            let error = install_into(project.path()).expect_err("must refuse");

            assert!(
                format!("{error:#}").contains(expected),
                "{text} must be refused as {expected}: {error:#}"
            );
        }
    }

    /// An empty file is one someone created and never filled in, which is an
    /// empty object rather than a parse failure to refuse over.
    #[test]
    fn empty_settings_are_treated_as_an_empty_object() {
        let project = tempfile::tempdir().expect("temporary directory");
        write_settings(project.path(), "\n  \n");

        install_into(project.path()).expect("installing");

        assert_eq!(commands_written(project.path()).len(), 2);
    }

    #[test]
    fn a_plan_that_writes_nothing_renders_what_is_already_there() {
        let project = tempfile::tempdir().expect("temporary directory");
        install_into(project.path()).expect("installing");

        let plan = plan(Scope::Project, project.path()).expect("planning");

        assert!(!plan.writes());
        assert!(
            plan.rendered()
                .expect("rendering")
                .contains("oboro hook pre-tool-use")
        );
    }

    /// The link can be the directory as easily as the file, and a component
    /// carrying a trailing slash would make the check follow it instead of
    /// seeing it.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_claude_directory_is_refused() {
        let project = tempfile::tempdir().expect("temporary directory");
        let elsewhere = project.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("creating the link target");
        std::os::unix::fs::symlink(&elsewhere, project.path().join(".claude")).expect("linking");

        let error = install_into(project.path()).expect_err("must refuse");

        assert!(format!("{error:#}").contains("symbolic link"), "{error:#}");
        assert!(
            !elsewhere.join("settings.local.json").exists(),
            "nothing is written through the link"
        );
    }

    #[test]
    fn a_command_naming_a_missing_path_is_not_reachable() {
        assert!(!program_is_reachable(
            "/nonexistent/oboro hook post-tool-use"
        ));
        assert!(!program_is_reachable(""));
    }
}
