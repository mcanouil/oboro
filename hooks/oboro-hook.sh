#!/usr/bin/env bash
# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil
#
# The hook command the Oboro plugin names, in place of `oboro hook <event>`.
#
# The plugin can install hooks; it cannot install the binary those hooks run.
# A missing binary would leave `PostToolUse` handing the model the raw file,
# which is the leak Oboro exists to stop, so this wrapper answers in the
# binary's place: the tool result is withheld, the tool call is refused, and
# the user is told what to install. Failing closed is the same decision the
# binary makes when it cannot clean (see `withheld_reply` and `refused_reply`
# in `src/main.rs`); only the reason differs.
#
# Both replies are printed on exit 0, because an agent only honours a hook's
# reply when the process exits 0. Nothing read from the payload is echoed
# back: the wrapper's messages are its own, so nothing a vault would redact
# can travel out through them.

set -euo pipefail

INSTALL="curl -fsSL https://m.canouil.dev/oboro/install.sh | bash"
NOT_INSTALLED="the oboro binary is not on PATH, so nothing can be anonymised. Install it with: ${INSTALL} (then run 'oboro doctor'), or disable the oboro plugin."

usage() {
	echo "Usage: oboro-hook.sh post-tool-use|pre-tool-use" >&2
	exit 2
}

# The payload is on standard input. It is never read here: the wrapper either
# hands the descriptor to the binary untouched, or answers without it. Draining
# it in the second case keeps the agent from seeing a write to a closed pipe.
drain() {
	cat >/dev/null 2>&1 || true
}

# Replaces the tool's result, since the tool has already run and there is no
# result left to prevent, only one to withhold.
withheld() {
	cat <<EOF
{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":"[oboro withheld this tool result: it could not be anonymised]"},"decision":"block","reason":"oboro is installed as a plugin but its binary is missing, so this tool result was withheld rather than shown unanonymised. The user has to install it before this tool can be used again.","systemMessage":"oboro withheld a tool result: ${NOT_INSTALLED}"}
EOF
}

# Refuses the call, since the tool has not run yet and letting it run would
# write a placeholder into the user's file.
refused() {
	cat <<EOF
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"oboro is installed as a plugin but its binary is missing, so this call was refused rather than allowed to write placeholders into a file. The user has to install it first."},"systemMessage":"oboro refused a tool call: ${NOT_INSTALLED}"}
EOF
}

main() {
	[ "$#" -eq 1 ] || usage
	case "$1" in
	post-tool-use | pre-tool-use) ;;
	*) usage ;;
	esac

	if command -v oboro >/dev/null 2>&1; then
		exec oboro hook "$1"
	fi

	drain
	case "$1" in
	post-tool-use) withheld ;;
	pre-tool-use) refused ;;
	esac
}

main "$@"
