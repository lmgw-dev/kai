#!/usr/bin/env bash
# Kai for Firefox's native manifest for a dev build (kai-firefox design §4.5). Firefox finds the
# native-messaging host dev.rootshell.kai through a per-user manifest, which it reads before the
# system folder (where the RPM's goes, /usr/lib64/mozilla/native-messaging-hosts/, the one Firefox
# 159 on Fedora reads: kai-firefox design §4.5):
#   ~/.mozilla/native-messaging-hosts/dev.rootshell.kai.json
# This one names this checkout's kai-browser-host (its canonical path), type stdio, and only Kai for
# Firefox (kai-firefox@rootshell.dev) as the extension that may start it; it is made from the same
# template as the RPM's (scripts/browser-manifest.sh). While it exists it shadows the RPM's
# manifest, and so the RPM's host, for every Firefox profile of this user; `remove` is the way back
# to the RPM's host.
#
# Written only into an existing ~/.mozilla: Firefox 147 and later keep a new user's profiles in the
# XDG folders while ~/.mozilla is missing (kai-firefox design §11, WP0 (e)), and making it could send
# such a Firefox back to the old folder. Without it the script stops and says so.
#
# One manifest per user names one host: install replaces another checkout's (and says whose);
# remove removes it only while its "path" names this binary (canonically), and leaves another
# checkout's, or one it cannot read, alone (exit 1). Where the RPM's system manifest is installed,
# install says that the per-user one shadows it for every profile of this user.
# KAI_BROWSER_SYSTEM_HOSTS (colon-separated folders, for scripts/browser-dev-check.sh) replaces the
# system folders Firefox reads.
#
# usage: scripts/browser-dev.sh install <host binary>   write the manifest (prints its path)
#        scripts/browser-dev.sh remove <host binary>    remove it while it names this binary
#        scripts/browser-dev.sh print <host binary>     the manifest's content, nothing written
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
name=dev.rootshell.kai
mozilla=$HOME/.mozilla
dir=$mozilla/native-messaging-hosts
file=$dir/$name.json
system_hosts=${KAI_BROWSER_SYSTEM_HOSTS:-/usr/lib64/mozilla/native-messaging-hosts:/usr/lib/mozilla/native-messaging-hosts}

die() { echo "browser-dev: $*" >&2; exit 1; }

# The manifest comes from the template the RPM's system manifest is made from too
# (packaging/dev.rootshell.kai.json.in through scripts/browser-manifest.sh): one source for the
# name, the type and the one extension that may start the host.
manifest() { # <canonical binary>
    "$here/browser-manifest.sh" "$1" "dev build $1"
}

# The path a manifest names (its "path", read as JSON); fails for a file that is no manifest.
named_path() { # <file>
    python3 -I -c 'import json, sys
path = json.load(open(sys.argv[1]))["path"]
assert isinstance(path, str)
print(path)' "$1" 2>/dev/null
}

[[ $# -eq 2 ]] || die "usage: scripts/browser-dev.sh install|remove|print <host binary>"
binary=$(realpath -m -- "$2")
[[ $binary != *[[:cntrl:]]* ]] || die "the path contains a control character (a newline?), which \
no manifest should carry: $binary"

case $1 in
    print)
        manifest "$binary"
        ;;
    install)
        [[ -x $binary ]] || die "no host binary at $binary (just browser-dev builds it)"
        [[ -d $mozilla ]] || die "$mozilla does not exist: this Firefox keeps its profiles in the \
XDG folders, and the manifest's place there is not confirmed yet (kai-firefox WP0 (e)); nothing written"
        if [[ -e $file ]]; then
            if ! named=$(named_path "$file"); then
                echo "browser-dev: replacing $file, which is no manifest this script can read" >&2
            elif [[ $(realpath -m -- "$named") != "$binary" ]]; then
                echo "browser-dev: replacing the manifest that named $named" >&2
            fi
        fi
        mkdir -p "$dir"
        tmp=$(mktemp "$dir/.$name.XXXXXX")
        trap 'rm -f -- "$tmp"' EXIT
        manifest "$binary" > "$tmp"
        chmod 644 "$tmp"
        mv -fT -- "$tmp" "$file"
        trap - EXIT
        echo "$file -> $binary"
        IFS=: read -r -a systems <<< "$system_hosts"
        for d in "${systems[@]}"; do
            if [[ -n $d && -e $d/$name.json ]]; then
                echo "browser-dev: note: the system manifest $d/$name.json is installed \
too; this per-user one shadows it for every Firefox profile of this user until just browser-dev-remove" >&2
            fi
        done
        ;;
    remove)
        if [[ ! -e $file ]]; then
            echo "browser-dev: no manifest at $file; nothing to remove"
        elif ! named=$(named_path "$file"); then
            die "$file is not a manifest this script can read (no \"path\" in JSON); left alone"
        elif [[ $(realpath -m -- "$named") == "$binary" ]]; then
            rm -f -- "$file"
            echo "browser-dev: removed $file"
            # The way back: what Firefox reads now is the RPM's system manifest, if there is one.
            IFS=: read -r -a systems <<< "$system_hosts"
            fallback=
            for d in "${systems[@]}"; do
                if [[ -n $d && -e $d/$name.json ]]; then
                    fallback=$d/$name.json
                    break
                fi
            done
            if [[ -z $fallback ]]; then
                echo "browser-dev: no system manifest is installed either: Firefox cannot start \
Kai's host until the kai RPM is installed or just browser-dev runs again"
            elif host=$(named_path "$fallback"); then
                echo "browser-dev: Firefox reads the system manifest $fallback now, which names \
$host"
            else
                echo "browser-dev: Firefox reads the system manifest $fallback now (it names \
no path this script can read)"
            fi
        else
            die "$file names $named, not $binary; left alone"
        fi
        ;;
    *)
        die "usage: scripts/browser-dev.sh install|remove|print <host binary>"
        ;;
esac
