//! The plugin this repository publishes: its manifests, and the hook command
//! it names.
//!
//! The plugin can install hooks; it cannot install the binary those hooks run.
//! So the wrapper it names has to answer in the binary's place when there is
//! none, and answer the way the binary would: a `PostToolUse` result withheld,
//! a `PreToolUse` call refused. A wrapper that let either through would turn
//! the plugin into a way of believing you are protected while nothing is.
//!
//! The manifests are read with `include_str!` rather than at run time, so a
//! file renamed or deleted is a build failure here rather than a plugin that
//! quietly stops loading for everyone who installs it.

/// The manifest an agent reads to load the plugin.
const PLUGIN: &str = include_str!("../.claude-plugin/plugin.json");
/// The manifest an agent reads to find the plugin.
const MARKETPLACE: &str = include_str!("../.claude-plugin/marketplace.json");
/// The hooks the plugin brings with it.
const HOOKS: &str = include_str!("../hooks/hooks.json");

fn parse(text: &str, what: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|error| panic!("{what} must be JSON: {error}"))
}

/// The version the plugin claims is what a user is offered an update to, so a
/// release that bumps the crate and leaves this behind offers them the release
/// before it.
#[test]
fn the_plugin_carries_this_build_s_version() {
    let plugin = parse(PLUGIN, "plugin.json");

    assert_eq!(
        plugin["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "plugin.json must carry the version in Cargo.toml"
    );
    assert_eq!(plugin["name"].as_str(), Some("oboro"));
}

/// `doctor` finds an enabled plugin by the name it goes by in `enabledPlugins`,
/// which is this manifest's name. Renamed here alone, the plugin would carry
/// the hooks while `doctor` went on saying nothing was installed.
#[test]
fn the_plugin_goes_by_the_name_doctor_looks_for() {
    let plugin = parse(PLUGIN, "plugin.json");

    assert_eq!(
        format!("{}@", plugin["name"].as_str().expect("a name")),
        oboro::hooks::PLUGIN_PREFIX
    );
}

/// The entry is what users type, and the source is what makes the skill in
/// `skills/oboro` the plugin's own rather than a copy of it.
#[test]
fn the_marketplace_offers_this_repository_as_the_plugin() {
    let marketplace = parse(MARKETPLACE, "marketplace.json");

    assert_eq!(marketplace["name"].as_str(), Some("oboro"));
    let plugins = marketplace["plugins"]
        .as_array()
        .expect("marketplace.json must list plugins");
    assert_eq!(plugins.len(), 1, "one plugin, listed once");
    assert_eq!(plugins[0]["name"].as_str(), Some("oboro"));
    assert_eq!(
        plugins[0]["source"].as_str(),
        Some("./"),
        "the plugin is this repository, so the skill and the hooks it ships are \
         the ones the binary carries"
    );
}

/// The same drift the skill's tests guard against, for the other file that
/// describes the hooks: a matcher widened in `hooks::EVENTS` and left alone
/// here would leave everyone who installed the plugin on the old one.
#[test]
fn the_plugin_hooks_match_the_events_oboro_answers() {
    let hooks = parse(HOOKS, "hooks/hooks.json");

    for event in oboro::hooks::EVENTS {
        let groups = hooks["hooks"][event.name]
            .as_array()
            .unwrap_or_else(|| panic!("hooks/hooks.json must name the {} hook", event.name));
        assert_eq!(groups.len(), 1, "one group per event");
        assert_eq!(
            groups[0]["matcher"].as_str(),
            Some(event.matcher),
            "the {} matcher must be the one oboro hook install writes",
            event.name
        );

        let command = groups[0]["hooks"][0]["command"]
            .as_str()
            .expect("a command");
        assert!(
            command.contains("${CLAUDE_PLUGIN_ROOT}"),
            "the command must be found wherever the plugin was installed: {command}"
        );
        assert!(
            command.contains("hooks/oboro-hook.sh") && command.ends_with(event.subcommand),
            "the {} hook must run the wrapper for {}: {command}",
            event.name,
            event.subcommand
        );
    }
}

/// Where the wrapper is, run from wherever the test happens to run.
// Only [`run`] calls this, and running a bash wrapper is Unix-only, so off Unix
// the helper is dead rather than merely unused.
#[cfg(unix)]
fn wrapper() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hooks/oboro-hook.sh")
}

/// Runs the wrapper with `path` as its `PATH`, feeding it `payload`.
#[cfg(unix)]
fn run(subcommand: &str, path: &str, payload: &str) -> std::process::Output {
    use std::io::Write as _;

    let mut child = std::process::Command::new(wrapper())
        .arg(subcommand)
        .env("PATH", path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("running the wrapper");
    child
        .stdin
        .take()
        .expect("a standard input")
        .write_all(payload.as_bytes())
        .expect("writing the payload");
    child.wait_with_output().expect("waiting for the wrapper")
}

/// A `PATH` holding the usual tools and no `oboro`, which is the state of a
/// machine where the plugin was installed and the binary never was.
#[cfg(unix)]
const WITHOUT_OBORO: &str = "/usr/bin:/bin";

#[cfg(unix)]
fn reply(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "a reply is only honoured on exit 0, and this exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    parse(
        &String::from_utf8(output.stdout.clone()).expect("the reply must be UTF-8"),
        "the reply",
    )
}

/// The tool has already run, so there is no call left to refuse: what the
/// model would have been shown is replaced instead.
#[cfg(unix)]
#[test]
fn a_result_is_withheld_when_the_binary_is_missing() {
    let output = run(
        "post-tool-use",
        WITHOUT_OBORO,
        r#"{"tool_name":"Read","tool_result":"call me on 06 12 34 56 78"}"#,
    );

    let reply = reply(&output);
    assert_eq!(
        reply["hookSpecificOutput"]["hookEventName"].as_str(),
        Some("PostToolUse")
    );
    assert_eq!(reply["decision"].as_str(), Some("block"));
    let shown = reply["hookSpecificOutput"]["updatedToolOutput"]
        .as_str()
        .expect("a replacement result");
    assert!(
        !shown.contains("06 12 34 56 78"),
        "the value must not survive into the replacement: {shown}"
    );
    assert!(
        reply["systemMessage"]
            .as_str()
            .is_some_and(|message| message.contains("install.sh")),
        "the user must be told what to install: {reply}"
    );
}

/// The tool has not run yet, so the write is stopped rather than reported
/// after it has happened.
#[cfg(unix)]
#[test]
fn a_call_is_refused_when_the_binary_is_missing() {
    let output = run(
        "pre-tool-use",
        WITHOUT_OBORO,
        r#"{"tool_name":"Write","tool_input":{"content":"[[PHONE_1]]"}}"#,
    );

    let reply = reply(&output);
    assert_eq!(
        reply["hookSpecificOutput"]["permissionDecision"].as_str(),
        Some("deny")
    );
    assert!(
        reply["systemMessage"]
            .as_str()
            .is_some_and(|message| message.contains("install.sh")),
        "the user must be told what to install: {reply}"
    );
}

/// With a binary to run, the wrapper is a name for it and nothing more: the
/// payload has to arrive as it was sent, or the hook would clean something
/// other than what the tool returned.
#[cfg(unix)]
#[test]
fn the_wrapper_hands_the_payload_to_the_binary_untouched() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let stub = directory.path().join("oboro");
    std::fs::write(&stub, "#!/bin/sh\nprintf '%s ' \"$@\"\ncat\n").expect("writing the stub");
    let mut permissions = std::fs::metadata(&stub)
        .expect("reading the stub")
        .permissions();
    {
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(0o755);
    }
    std::fs::set_permissions(&stub, permissions).expect("making the stub runnable");

    let path = format!("{}:{WITHOUT_OBORO}", directory.path().display());
    let output = run("post-tool-use", &path, r#"{"tool_result":"kept"}"#);

    assert_eq!(
        String::from_utf8(output.stdout).expect("the output must be UTF-8"),
        r#"hook post-tool-use {"tool_result":"kept"}"#,
        "the wrapper must run `oboro hook post-tool-use` and pass the payload through"
    );
}

/// The wrapper's replies are the binary's replies, written out by hand because
/// the binary is absent exactly when they are needed. Nothing anchors the two
/// but this: an agent that renamed a field would have the Rust side updated and
/// the shell side left describing the old protocol, and a stale `PostToolUse`
/// reply is the raw result reaching the model.
///
/// The binary is driven into the same corner the way `tests/hook.rs` does it, by
/// breaking the vault, so what is compared is two answers to one question.
#[cfg(unix)]
#[test]
fn the_wrapper_answers_in_the_shape_the_binary_answers() {
    for (subcommand, payload, decisive) in [
        (
            "post-tool-use",
            r#"{"tool_name":"Read","tool_result":"call me on 06 12 34 56 78"}"#,
            "decision",
        ),
        (
            "pre-tool-use",
            r#"{"tool_name":"Write","tool_input":{"content":"[[PHONE_1]]"}}"#,
            "hookSpecificOutput.permissionDecision",
        ),
    ] {
        let theirs = reply(&run(subcommand, WITHOUT_OBORO, payload));
        let ours = broken_binary_reply(subcommand, payload);

        assert_eq!(
            keys(&theirs),
            keys(&ours),
            "the {subcommand} wrapper reply must carry the fields the binary's does"
        );
        assert_eq!(
            keys(&theirs["hookSpecificOutput"]),
            keys(&ours["hookSpecificOutput"]),
            "the {subcommand} wrapper's hookSpecificOutput must match the binary's"
        );
        let value = |reply: &serde_json::Value| {
            decisive
                .split('.')
                .fold(reply.clone(), |value, key| value[key].clone())
        };
        assert_eq!(
            value(&theirs),
            value(&ours),
            "the {subcommand} wrapper must fail closed the way the binary does"
        );
    }
}

/// The field names of a JSON object, sorted, for comparing two shapes.
#[cfg(unix)]
fn keys(value: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<_> = value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// What the binary replies when it cannot do its job, obtained by pointing it
/// at a vault it cannot open.
#[cfg(unix)]
fn broken_binary_reply(subcommand: &str, payload: &str) -> serde_json::Value {
    use std::io::Write as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let vault = directory.path().join("vault.db");
    std::fs::create_dir(&vault).expect("creating a directory where a vault is expected");

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("oboro"))
        .current_dir(directory.path())
        .arg("--vault")
        .arg(&vault)
        .arg("--key")
        .arg(directory.path().join("key"))
        .arg("hook")
        .arg(subcommand)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("running oboro");
    child
        .stdin
        .take()
        .expect("a standard input")
        .write_all(payload.as_bytes())
        .expect("writing the payload");
    let output = child.wait_with_output().expect("waiting for oboro");

    reply(&output)
}

/// The wrapper is named by the plugin's own manifest, so a subcommand it does
/// not know is a manifest that has drifted rather than a user's mistake, and
/// failing loudly is how that gets noticed.
#[cfg(unix)]
#[test]
fn an_unknown_subcommand_is_refused() {
    let output = run("post-tool-used", WITHOUT_OBORO, "{}");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "nothing is replied to the agent");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Usage"),
        "the reason is said out loud"
    );
}
