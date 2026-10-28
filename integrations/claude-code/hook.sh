#!/bin/bash
# Claude Code PreToolUse(Bash) hook: heavy commands go through machine-slot.
# A bare cargo build/test/run/check/clippy/bench/nextest, xcodebuild,
# swift build/test or hyperfine is refused with the line to use instead;
# benches (cargo bench, hyperfine, --bench) must be `solo`. Text inside
# quotes (commit messages, echo) is ignored.
#
# Tools that take their own turns can be let through with `hook_allow`
# in ~/.config/machine-slot/config, comma-separated: hook_allow = mybuild,other
input="$(cat)"
cmd="$(jq -r '.tool_input.command // ""' <<<"$input")"
cmd="$(sed -E "s/\"[^\"]*\"//g; s/'[^']*'//g" <<<"$cmd")"
deny() {
    jq -n --arg r "$1" '{hookSpecificOutput: {hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: $r}}'
    exit 0
}

config="${XDG_CONFIG_HOME:-$HOME/.config}/machine-slot/config"
allow="$(sed -nE 's/^[[:space:]]*hook_allow[[:space:]]*=[[:space:]]*"?([^"#]*)"?.*/\1/p' "$config" 2>/dev/null | tr -d ' ')"
word='(^|[;&|(`[:space:]])'
IFS=',' read -ra allowed <<<"$allow"
for a in "${allowed[@]}"; do
    [ -n "$a" ] && grep -qE "${word}${a}([[:space:]]|$)" <<<"$cmd" && exit 0
done

heavy="${word}(cargo( \+[^ ]+)? (test|bench|build|run|check|clippy|nextest|criterion)|xcodebuild|swift (build|test)|hyperfine)([[:space:]]|$)"
timed="${word}(cargo( \+[^ ]+)? (bench|criterion)|hyperfine)([[:space:]]|$)|--bench([[:space:]=]|$)"
grep -qE "$heavy" <<<"$cmd" || exit 0

grep -qE "${word}machine-slot +solo " <<<"$cmd" && exit 0
if grep -qE "$timed" <<<"$cmd"; then
    deny "Measurements run alone on this machine: \`machine-slot solo [--needs cpu,disk,ram,gpu,screen] \"<project: what you measure>\" -- <command>\` (default cpu,disk,ram; add gpu for GPU work, screen for UI tests). It waits for its turn and a quiet machine, so run it in the background. Use the same label for the same task. See \`machine-slot --help\`."
fi
grep -qE "${word}machine-slot +shared " <<<"$cmd" && exit 0
deny "Heavy work goes through the machine queue: \`machine-slot shared \"<project: task>\" -- <command>\` (runs alongside other work, never during a measurement). To measure time or resources use \`machine-slot solo [--needs cpu,disk,ram,gpu,screen] ...\`. Same label for the same task; run it in the background if it may wait. \`machine-slot status\` / \`machine-slot estimate LABEL\` show the queue."
