//! Finding out whether the agent hooks are actually installed.
//!
//! The hooks are documented and named in the user's own settings rather than
//! written there by Oboro, which means a user can believe they are protected
//! while nothing is wired up. `oboro doctor` closes that gap by reporting what
//! it can see, so protection is something to verify rather than assume.

use std::path::{Path, PathBuf};

/// The two events Oboro answers, with the subcommand each is answered by.
pub const EVENTS: [(&str, &str); 2] = [
    ("PostToolUse", "post-tool-use"),
    ("PreToolUse", "pre-tool-use"),
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
        for (event, subcommand) in EVENTS {
            for (command, matcher) in commands_for(&settings, event) {
                if names_oboro_hook(&command, subcommand) {
                    found.push(Installed {
                        event,
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

    #[test]
    fn a_command_naming_a_missing_path_is_not_reachable() {
        assert!(!program_is_reachable(
            "/nonexistent/oboro hook post-tool-use"
        ));
        assert!(!program_is_reachable(""));
    }
}
