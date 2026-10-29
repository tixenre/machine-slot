#!/bin/sh
# Claude Code PreToolUse(Bash) hook: heavy commands go through machine-slot.
# The check itself is `machine-slot hook` (fast, no jq); this only finds the
# binary, which hooks may not have on their PATH. Without it, or if it
# fails, nothing is refused.
#
# The lists grow from ~/.config/machine-slot/config, each comma-separated:
#   hook_heavy = ./build.sh, just test    more heavy commands
#   hook_timed = ./bench.sh               more commands that measure
#   hook_allow = mybuild                  tools that take their own turns
for b in "$(command -v machine-slot)" "$HOME/.cargo/bin/machine-slot" "$HOME/.local/bin/machine-slot" \
    /opt/homebrew/bin/machine-slot /usr/local/bin/machine-slot; do
    # Never block on our own failure (an older binary without `hook`).
    [ -x "$b" ] && { "$b" hook 2>/dev/null; exit 0; }
done
exit 0
