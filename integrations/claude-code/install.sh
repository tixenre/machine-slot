#!/bin/bash
# Sets up machine-slot for Claude Code, for the current user:
#   - the skill, in ~/.claude/skills/machine-slot;
#   - the PreToolUse hook, added to ~/.claude/settings.json (backed up first);
#   - the read-only MCP server, through `claude mcp add` (when the CLI is there).
# Run it again to update; --uninstall removes all three.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
claude_dir="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
settings="$claude_dir/settings.json"
hook="$claude_dir/hooks/machine-slot.sh"

command -v jq >/dev/null || { echo "needs jq (brew install jq)" >&2; exit 1; }

if [ "${1:-}" = --uninstall ]; then
    rm -rf "$claude_dir/skills/machine-slot" "$hook"
    if [ -f "$settings" ]; then
        cp -p "$settings" "$settings.bak-machine-slot"
        jq '(.hooks.PreToolUse // []) |= map(.hooks |= map(select(.command | test("machine-slot") | not))) | .hooks.PreToolUse |= map(select(.hooks | length > 0))' \
            "$settings.bak-machine-slot" > "$settings"
    fi
    command -v claude >/dev/null && claude mcp remove --scope user machine-slot >/dev/null 2>&1 || true
    echo "machine-slot removed from Claude Code (settings backup: $settings.bak-machine-slot)"
    exit 0
fi

command -v machine-slot >/dev/null || echo "note: machine-slot is not on PATH yet (cargo install --path . or a release binary)" >&2

mkdir -p "$claude_dir/skills/machine-slot" "$claude_dir/hooks"
cp "$here/skill/SKILL.md" "$claude_dir/skills/machine-slot/SKILL.md"
cp "$here/hook.sh" "$hook"
chmod +x "$hook"

[ -f "$settings" ] || echo '{}' > "$settings"
cp -p "$settings" "$settings.bak-machine-slot"
jq --arg cmd "$hook" '
  .hooks.PreToolUse = (.hooks.PreToolUse // [])
  | if any(.hooks.PreToolUse[]; any(.hooks[]?; .command == $cmd)) then .
    else .hooks.PreToolUse += [{matcher: "Bash", hooks: [{type: "command", command: $cmd, timeout: 10}]}] end
' "$settings.bak-machine-slot" > "$settings"

if command -v claude >/dev/null && command -v machine-slot >/dev/null; then
    claude mcp remove --scope user machine-slot >/dev/null 2>&1 || true
    claude mcp add --scope user machine-slot -- "$(command -v machine-slot)" mcp >/dev/null && echo "MCP server registered (user scope)"
fi
echo "machine-slot set up for Claude Code: skill, hook (settings backup: $settings.bak-machine-slot)"
