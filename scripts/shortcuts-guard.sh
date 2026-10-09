# shellcheck shell=bash
# Sourced by every script that starts Kai, except the two that check the shortcuts themselves
# (scripts/live-check-shortcuts.sh and scripts/owner-check-shortcuts.sh): a Kai a check starts
# registers no global shortcuts with Plasma (its configuration sets shortcuts.bind = false), and the
# owner's kglobalshortcutsrc gains no [dev.rootshell.kai] group. A Kai that registered once leaves an
# empty group there, which spoils later shortcut checks.
#
# Source it before the script changes XDG_CONFIG_HOME: the file is the session's, which KGlobalAccel
# writes. Then:
#   kai_config <file> [toml text]   writes Kai's configuration: the text, then shortcuts.bind = false
#   kgs_check                        PASS, or FAIL (status 1) when a [dev.rootshell.kai…] group came
# and finish the script's EXIT trap with `kgs_check || exit 1`.

KGS_RC=${XDG_CONFIG_HOME:-$HOME/.config}/kglobalshortcutsrc

kgs_groups() {
    [[ -f $KGS_RC ]] || return 0
    grep -E '^\[dev\.rootshell\.kai[^]]*\]' "$KGS_RC" | sort -u || true
}

KGS_BEFORE=$(kgs_groups)

kai_config() {
    local file=$1 text=${2:-}
    mkdir -p "$(dirname "$file")"
    if [[ $text == *"[shortcuts]"* ]]; then
        printf '%b' "$text" | sed 's/^\[shortcuts\]$/[shortcuts]\nbind = false/' > "$file"
    else
        printf '%b[shortcuts]\nbind = false\n' "$text" > "$file"
    fi
}

kgs_check() {
    # KGlobalAccel writes its file shortly after a change.
    sleep 1
    local new
    new=$(comm -13 <(printf '%s\n' "$KGS_BEFORE") <(kgs_groups) | sed '/^$/d')
    if [[ -n $new ]]; then
        echo "FAIL: $KGS_RC gained $(echo "$new" | paste -sd' '): a Kai of this check registered its shortcuts" >&2
        return 1
    fi
    echo "PASS: $KGS_RC gained no [dev.rootshell.kai] group"
}
