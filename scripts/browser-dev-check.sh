#!/usr/bin/env bash
# Kai for Firefox's dev manifest (scripts/browser-dev.sh) in a scratch HOME: nothing is written to
# the owner's ~/.mozilla. Without ~/.mozilla nothing is written and the folder is not made. With
# it, the manifest is valid JSON naming dev.rootshell.kai, stdio, Kai for Firefox's id and the
# binary's canonical path (a symlinked checkout resolves), mode 0644, and a path with a quote or a
# backslash stays valid JSON while a control character is refused. A second checkout's install
# replaces it and says whose; remove leaves another checkout's manifest (and one it cannot read)
# alone and removes its own by the "path" it names, however the file is written. A failed install
# leaves no temporary file. Beside a system manifest (a scratch folder through
# KAI_BROWSER_SYSTEM_HOSTS) install notes that the per-user one shadows it, and not without one,
# and remove says which host Firefox reads next: the system manifest's, or none. The dev manifest
# and the RPM's system manifest come from one template (scripts/browser-manifest.sh): the same
# fields but the path and the description, the `&` and the other placeholder's name in a path kept.
# usage: scripts/browser-dev-check.sh   (exit 1 on a failed check)
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
dev=$here/scripts/browser-dev.sh
manifest=$here/scripts/browser-manifest.sh
scratch=$(mktemp -d "${TMPDIR:-/tmp}/kai-browser-dev-check.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export HOME=$scratch/home
export KAI_BROWSER_SYSTEM_HOSTS=$scratch/system-hosts
mkdir -p "$HOME" "$KAI_BROWSER_SYSTEM_HOSTS"
file=$HOME/.mozilla/native-messaging-hosts/dev.rootshell.kai.json
failed=0
check() { # <ok?> <what>
    if [[ $1 == 0 ]]; then echo "ok   $2"; else echo "FAIL $2"; failed=1; fi
}
build() { # <checkout>: a fake build's host binary
    mkdir -p "$1/target/debug"
    printf '#!/bin/sh\n' > "$1/target/debug/kai-browser-host"
    chmod +x "$1/target/debug/kai-browser-host"
}
# The manifest's fields as python reads them: name, type, path, the allowed extensions.
fields() { # <file>
    python3 -I -c 'import json, sys
m = json.load(open(sys.argv[1]))
print(m["name"], m["type"], ",".join(m["allowed_extensions"]), sep="|")
print(m["path"])
print(bool(m["description"]))' "$1"
}

for w in kai kai-wt-feature 'kai "quoted" \back'; do build "$scratch/$w"; done
ln -s "$scratch/kai-wt-feature" "$scratch/linked"
main=$scratch/kai/target/debug/kai-browser-host
feature=$scratch/kai-wt-feature/target/debug/kai-browser-host

out=$("$dev" install "$main" 2>&1) && rc=0 || rc=$?
check $(( rc != 0 ? 0 : 1 )) "without ~/.mozilla install refuses"
check $([[ $out == *"does not exist"* && ! -e $HOME/.mozilla ]]; echo $?) \
    "and says why, making no ~/.mozilla ($out)"

mkdir "$HOME/.mozilla"
out=$("$dev" install "$main" 2>&1 >/dev/null)
check $([[ -f $file ]]; echo $?) "install writes $file"
check $([[ -z $out ]]; echo $?) "without a system manifest nothing more is said ($out)"
check $([[ $(stat -c %a "$file") == 644 ]]; echo $?) "mode 0644"
check $([[ $(fields "$file") == "dev.rootshell.kai|stdio|kai-firefox@rootshell.dev
$main
True" ]]; echo $?) "the manifest names the host, stdio, Kai for Firefox and the binary"

out=$("$dev" install "$scratch/linked/target/debug/kai-browser-host" 2>&1 >/dev/null)
check $([[ $(fields "$file" | sed -n 2p) == "$feature" ]]; echo $?) \
    "a symlinked checkout's install names the canonical path"
check $([[ $out == *"replacing the manifest that named $main"* ]]; echo $?) \
    "and says whose it replaced ($out)"
check $([[ -z $(ls -A "$HOME/.mozilla/native-messaging-hosts" | grep -v '^dev.rootshell.kai.json$') ]]
    echo $?) "no temporary file is left"

out=$("$dev" remove "$main" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && -f $file && $out == *"left alone"* ]]; echo $?) \
    "remove leaves another checkout's manifest ($out)"
"$dev" remove "$feature" > /dev/null
check $([[ ! -e $file ]]; echo $?) "remove takes its own"
check $([[ $("$dev" remove "$feature") == *"nothing to remove"* ]]; echo $?) \
    "remove with nothing there says so"

odd=$scratch/'kai "quoted" \back'/target/debug/kai-browser-host
"$dev" install "$odd" > /dev/null
check $([[ $(fields "$file" | sed -n 2p) == "$odd" ]]; echo $?) \
    "a path with a quote and a backslash stays valid JSON"
"$dev" remove "$odd" > /dev/null
check $([[ ! -e $file ]]; echo $?) "and is removed as its own"
nl=$scratch/$'new\nline'
build "$nl"
out=$("$dev" install "$nl/target/debug/kai-browser-host" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && ! -e $file && $out == *"control character"* ]]; echo $?) \
    "a path with a newline is refused"
out=$("$dev" install "$scratch/none/target/debug/kai-browser-host" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && $out == *"no host binary"* ]]; echo $?) "a missing binary is refused"

# remove goes by the path the manifest names, not by its bytes: one written another way (one line,
# another description, the path through the symlink) is still this checkout's.
printf '{"name":"dev.rootshell.kai","description":"older","path":"%s","type":"stdio","allowed_extensions":["kai-firefox@rootshell.dev"]}' \
    "$scratch/linked/target/debug/kai-browser-host" > "$file"
"$dev" remove "$feature" > /dev/null
check $([[ ! -e $file ]]; echo $?) "remove takes its own manifest however it is written"
printf '{"path":"%s"}' "$main" > "$file"
out=$("$dev" remove "$feature" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && -f $file && $out == *"names $main"* ]]; echo $?) \
    "and leaves one naming another binary ($out)"
printf 'not json' > "$file"
out=$("$dev" remove "$feature" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && -f $file && $out == *"can read"* ]]; echo $?) \
    "and one it cannot read ($out)"
rm -f -- "$file"

# A failed install (a directory in the manifest's place) leaves no temporary file behind.
mkdir -p "$file/in-the-way"
out=$("$dev" install "$main" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && -d $file ]]; echo $?) "an install that cannot replace a directory fails ($out)"
check $([[ -z $(ls -A "$HOME/.mozilla/native-messaging-hosts" | grep -v '^dev.rootshell.kai.json$') ]]
    echo $?) "and leaves no temporary file"
rm -rf -- "$file"

# Beside the RPM's system manifest, install says the per-user one shadows it.
printf '{}' > "$KAI_BROWSER_SYSTEM_HOSTS/dev.rootshell.kai.json"
out=$("$dev" install "$main" 2>&1 >/dev/null)
check $([[ -f $file && $out == *"$KAI_BROWSER_SYSTEM_HOSTS/dev.rootshell.kai.json"* \
    && $out == *"shadows it for every Firefox profile"* ]]; echo $?) \
    "beside a system manifest install notes the shadowing ($out)"

# The RPM's system manifest (packaging/kai.spec's %install) and the dev one are made by the same
# script from the same template: only the path and the description differ.
system_host=/usr/libexec/kai/kai-browser-host
"$manifest" "$system_host" > "$scratch/system.json"
check $([[ $(fields "$scratch/system.json") == "dev.rootshell.kai|stdio|kai-firefox@rootshell.dev
$system_host
True" ]]; echo $?) "the system manifest names $system_host, stdio and Kai for Firefox"
check $([[ $("$dev" print "$main") == "$("$manifest" "$main" "dev build $main")" ]]; echo $?) \
    "browser-dev's manifest is browser-manifest.sh's"
blank() { sed -E 's#"(path|description)": "[^"]*"#"\1": X#'; }
check $([[ -n $(blank < "$scratch/system.json") \
    && $(blank < "$scratch/system.json") == "$("$dev" print "$main" | blank)" ]]; echo $?) \
    "the two differ in their path and description only"
amp=$scratch/'a & @HOST@ @DESCRIPTION@'/target/debug/kai-browser-host
check $([[ $("$manifest" "$amp" | python3 -I -c 'import json, sys
print(json.load(sys.stdin)["path"])') == "$amp" ]]; echo $?) \
    "a path with & and the placeholders' names comes out as it is"
out=$("$manifest" relative/host 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && $out == *"not absolute"* ]]; echo $?) "a relative host path is refused"

# remove says which host Firefox reads next.
"$dev" install "$main" > /dev/null 2>&1
rm -f -- "$KAI_BROWSER_SYSTEM_HOSTS/dev.rootshell.kai.json"
out=$("$dev" remove "$main" 2>&1)
check $([[ $out == *"no system manifest is installed either"* ]]; echo $?) \
    "remove without a system manifest says Firefox has no host ($out)"
"$dev" install "$main" > /dev/null 2>&1
cp "$scratch/system.json" "$KAI_BROWSER_SYSTEM_HOSTS/dev.rootshell.kai.json"
out=$("$dev" remove "$main" 2>&1)
check $([[ $out == *"Firefox reads the system manifest $KAI_BROWSER_SYSTEM_HOSTS/dev.rootshell.kai.json now, which names $system_host"* ]]
    echo $?) "remove beside a system manifest says Firefox reads it now ($out)"

exit $failed
