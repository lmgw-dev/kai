#!/usr/bin/env bash
# Kai for Firefox's native-messaging manifest (kai-firefox design §4.5) from its one template,
# packaging/dev.rootshell.kai.json.in: the name dev.rootshell.kai, type stdio and the one extension
# that may start the host (kai-firefox@rootshell.dev). Both places a manifest comes from use it:
# the RPM's %install (the system manifest naming /usr/libexec/kai/kai-browser-host) and
# scripts/browser-dev.sh (the per-user manifest naming a checkout's debug host).
# Prints the manifest on stdout; the path is escaped as a JSON string, whatever it holds but a
# control character, which is refused.
# usage: scripts/browser-manifest.sh <absolute host path> [note]   (the note goes in the description)
set -euo pipefail

template=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packaging/dev.rootshell.kai.json.in

die() { echo "browser-manifest: $*" >&2; exit 1; }

# A string's content as JSON: \ and " escaped (a control character is refused before, below).
json_string() { # <text>
    local s=${1//\\/\\\\}
    printf '%s' "${s//\"/\\\"}"
}

[[ $# -ge 1 && $# -le 2 ]] || die "usage: scripts/browser-manifest.sh <absolute host path> [note]"
host=$1
[[ $host == /* ]] || die "the host's path is not absolute: $host"
[[ $host != *[[:cntrl:]]* ]] || die "the path contains a control character (a newline?), which \
no manifest should carry: $host"
description="Kai for Firefox's link to Kai"
if [[ -n ${2:-} ]]; then
    [[ $2 != *[[:cntrl:]]* ]] || die "the note contains a control character"
    description+=" ($2)"
fi
description=$(json_string "$description")
host=$(json_string "$host")

# One placeholder per template line, each replaced alone (a path that spells the other placeholder
# stays as it is). The replacements are quoted: `&` in them is no match reference.
while IFS= read -r line || [[ -n $line ]]; do
    case $line in
        *@DESCRIPTION@*) line=${line/'@DESCRIPTION@'/"$description"} ;;
        *@HOST@*) line=${line/'@HOST@'/"$host"} ;;
    esac
    printf '%s\n' "$line"
done < "$template"
