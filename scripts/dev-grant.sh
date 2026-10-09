#!/usr/bin/env bash
# A dev build's grant of ruling 16's restricted interfaces (§16 Q21, the owner's answer of
# 2026-10-09: opt-in per build). KWin grants them to the binary whose canonical absolute path a
# .desktop file in the user's applications directory names (WP13 (d): symlinks resolve, a PATH
# lookup does not; new connections get the grant about 1.4 s after the file appears, and lose it
# when it goes). Each checkout's or worktree's binary has its own path, so each gets its own file:
#   <applications>/dev.rootshell.kai-devgrant-<checkout dir>-<8 hex of the path's sha256>.desktop
# (both from canonical paths: a symlinked and the real checkout name the same file) with the two
# keys copied from packaging/dev.rootshell.kai.desktop.in, NoDisplay=true and no MimeType (it is no
# scheme handler and no menu entry). `just install-dev` stays without the keys.
#
# One entry per binary: KWin (serviceutils.h, fetchProcessServiceField) takes the first service
# KApplicationTrader lists whose Exec's first word (QProcess::splitCommand) resolves to the
# binary, in the service cache's order. With install-dev's stripped entry and a grant naming the
# same binary, whether the grant held would be chance, so grant refuses while another entry names
# the binary without the keys (and says which), `installable` (install-dev's guard) refuses while
# a grant names the binary, and list marks a grant such an entry shadows.
#
# usage: scripts/dev-grant.sh grant <binary> [checkout]   write this build's file (prints its path)
#        scripts/dev-grant.sh revoke <binary> [checkout]  remove this build's grant file(s)
#        scripts/dev-grant.sh list                        every dev grant file, its binary, and
#                                                          whether that still exists or is shadowed
#                                                          (stale ones are left for the owner)
#        scripts/dev-grant.sh print <binary> [checkout]   the file's name and content, nothing written
#        scripts/dev-grant.sh installable <binary>        exit 1, saying why, while a grant names it
# The applications directory is $XDG_DATA_HOME/applications (default ~/.local/share/applications);
# entries that could shadow a grant are searched there and in $XDG_DATA_DIRS' applications.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
template=$here/packaging/dev.rootshell.kai.desktop.in
apps=${XDG_DATA_HOME:-$HOME/.local/share}/applications
prefix=dev.rootshell.kai-devgrant-
marker=X-Kai-Dev-Grant
key_re='^X-KDE-(Wayland|DBUS-Restricted)-Interfaces='

die() { echo "dev-grant: $*" >&2; exit 1; }

# The canonical absolute path KWin compares against (the binary need not exist for revoke).
canonical() { realpath -m -- "$1"; }

# The restricted-interface keys a grant carries, as the packaged .desktop has them.
keys() {
    local k
    k=$(grep -E "$key_re" "$template") || die "no restricted-interface keys in $template"
    [[ $(wc -l <<< "$k") -eq 2 ]] || die "expected the two restricted-interface keys in $template"
    printf '%s\n' "$k"
}

# A path the Exec key can carry so that the Desktop Entry spec and KWin read the same path. It is
# written in double quotes, where spaces and the spec's other reserved characters are literal.
# Inside quotes the spec wants \" \` \$ \\ escaped and % written %%, but KService hands KWin the
# Exec value with neither undone and QProcess::splitCommand undoes neither, so a path with one of
# those could not match: it is refused, as is a control character (a newline would end the key).
exec_safe() { # <path>
    local bad
    [[ $1 != *[[:cntrl:]]* ]] \
        || die "the path contains a control character (a newline?), no .desktop can name it: $1"
    bad=$(tr -dc '"`$\\%' <<< "$1")
    [[ -z $bad ]] || die "the path contains $bad, which a .desktop Exec must escape while KWin \
compares it unescaped, so no grant could match: $1"
}

# The file's name for a binary: the checkout's directory name and a short hash of the binary's path.
file_name() { # <canonical binary> <canonical checkout>
    local tag hash
    tag=$(basename -- "$2" | tr -c 'A-Za-z0-9_.\n-' '_')
    hash=$(printf '%s' "$1" | sha256sum | cut -c1-8)
    printf '%s%s-%s.desktop' "$prefix" "$tag" "$hash"
}

content() { # <canonical binary> <canonical checkout>
    local name
    name=$(basename -- "$2" | tr '[:cntrl:]' '_')
    cat <<EOF
[Desktop Entry]
Type=Application
Name=Kai dev grant ($name)
Comment=Grants ruling 16's restricted interfaces to the Kai dev build at $1 (just grant-dev; just revoke-dev removes it)
Exec="$1"
NoDisplay=true
Terminal=false
$marker=$1
$(keys)
EOF
}

# A key's value in a .desktop file's [Desktop Entry] group, with KConfig's escapes undone.
entry() { # <file> <key>
    local raw out='' i c
    raw=$(awk -v key="$2" '
        /^\[/ { group = $0; next }
        group == "[Desktop Entry]" && index($0, key "=") == 1 {
            print substr($0, length(key) + 2); exit
        }' "$1" 2>/dev/null) || return 0
    for ((i = 0; i < ${#raw}; i++)); do
        c=${raw:i:1}
        if [[ $c == '\' ]] && ((i + 1 < ${#raw})); then
            ((++i))
            case ${raw:i:1} in
                s) out+=' ' ;; t) out+=$'\t' ;; n) out+=$'\n' ;; r) out+=$'\r' ;;
                *) out+=${raw:i:1} ;;
            esac
        else
            out+=$c
        fi
    done
    printf '%s' "$out"
}

# The first word of a command as QProcess::splitCommand splits it (KWin's reading of Exec):
# double quotes group, three in a row are one literal quote, a backslash is a plain character.
first_word() { # <command>
    local s=$1 out='' quotes=0 inquote=0 i c
    for ((i = 0; i < ${#s}; i++)); do
        c=${s:i:1}
        if [[ $c == '"' ]]; then
            ((++quotes))
            if ((quotes == 3)); then quotes=0; out+='"'; fi
            continue
        fi
        if ((quotes)); then
            ((quotes == 1)) && inquote=$((!inquote))
            quotes=0
        fi
        if ((!inquote)) && [[ $c == [[:space:]] ]]; then
            [[ -z $out ]] || break
        else
            out+=$c
        fi
    done
    printf '%s' "$out"
}

# Every other .desktop KWin could read for this binary without the grant's two keys, one per line.
# The binary's own dev grant files are not among them (grant replaces them, revoke removes them).
shadowing() { # <canonical binary>
    local dirs=("$apps") extra d f word wanted
    IFS=: read -r -a extra <<< "${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
    for d in "${extra[@]}"; do [[ -z $d ]] || dirs+=("$d/applications"); done
    wanted=$(keys | sort)
    for d in "${dirs[@]}"; do
        [[ -d $d ]] || continue
        while IFS= read -r -d '' f; do
            [[ $(entry "$f" "$marker") != "$1" ]] || continue
            [[ $(entry "$f" Hidden) != true ]] || continue
            word=$(first_word "$(entry "$f" Exec)")
            [[ $word == /* && $(canonical "$word") == "$1" ]] || continue
            [[ $( (grep -E "$key_re" "$f" || true) | sort) != "$wanted" ]] || continue
            printf '%s\n' "$f"
        done < <(find "$d" -name '*.desktop' -print0 2>/dev/null)
    done
}

# This binary's dev grant files, whatever their name (an older one named through a symlink).
grant_files() { # <canonical binary>
    local f
    shopt -s nullglob
    for f in "$apps/$prefix"*.desktop; do
        [[ $(entry "$f" "$marker") != "$1" ]] || printf '%s\n' "$f"
    done
}

cmd=${1:-}
case $cmd in
    grant|revoke|print)
        [[ $# -ge 2 ]] || die "usage: $0 $cmd <binary> [checkout]"
        bin=$(canonical "$2")
        checkout=$(canonical "${3:-$here}")
        name=$(file_name "$bin" "$checkout")
        ;;
    installable)
        [[ $# -ge 2 ]] || die "usage: $0 installable <binary>"
        bin=$(canonical "$2")
        ;;
esac
case $cmd in
    print)
        exec_safe "$bin"
        echo "$name"
        content "$bin" "$checkout"
        ;;
    grant)
        [[ -x $bin ]] || die "$bin is not an executable: build it first"
        exec_safe "$bin"
        mapfile -t others < <(shadowing "$bin")
        if [[ ${#others[@]} -gt 0 ]]; then
            {
                echo "dev-grant: not granted: another .desktop names $bin without the keys:"
                printf '  %s\n' "${others[@]}"
                echo "KWin reads the first entry for a binary that its service cache lists, so with"
                echo "both the grant would hold or not by chance. Remove that entry first (install-dev's:"
                echo "just uninstall-dev), then grant again; install-dev refuses while a grant exists."
            } >&2
            exit 1
        fi
        install -d "$apps"
        tmp=$(mktemp "$apps/.${name}.XXXXXX")
        content "$bin" "$checkout" > "$tmp"
        chmod 644 "$tmp"
        mv -f -- "$tmp" "$apps/$name"
        while IFS= read -r f; do
            [[ $f != "$apps/$name" ]] || continue
            rm -f -- "$f"
            echo "replaced: $f (this binary's grant under another name)"
        done < <(grant_files "$bin")
        echo "granted: $apps/$name -> $bin"
        echo "KWin grants new connections about 1.4 s after the file appears: start Kai after that."
        ;;
    revoke)
        mapfile -t files < <(grant_files "$bin")
        if [[ ${#files[@]} -eq 0 ]]; then
            echo "no grant for $bin ($apps/$name)"
        else
            for f in "${files[@]}"; do
                rm -f -- "$f"
                echo "revoked: removed $f"
            done
            echo "a running Kai keeps what its connections were granted until it reconnects"
        fi
        ;;
    installable)
        mapfile -t files < <(grant_files "$bin")
        if [[ ${#files[@]} -gt 0 ]]; then
            {
                echo "dev-grant: a dev grant names $bin:"
                printf '  %s\n' "${files[@]}"
                echo "an entry without the keys beside it would make the grant hold by chance (KWin"
                echo "reads the first entry for a binary): just revoke-dev first, or keep the grant."
            } >&2
            exit 1
        fi
        ;;
    list)
        shopt -s nullglob
        files=("$apps/$prefix"*.desktop)
        if [[ ${#files[@]} -eq 0 ]]; then
            echo "no Kai dev grants in $apps"
            exit 0
        fi
        for f in "${files[@]}"; do
            bin=$(entry "$f" "$marker")
            [[ -n $bin ]] || bin=$(first_word "$(entry "$f" Exec)")
            if [[ ! -x $bin ]]; then
                state="STALE (binary gone: a removed worktree or build?)"
            else
                mapfile -t others < <(shadowing "$(canonical "$bin")")
                if [[ ${#others[@]} -gt 0 ]]; then
                    state="SHADOWED by ${others[*]} (it names the binary without the keys: KWin may read either)"
                else
                    state=ok
                fi
            fi
            printf '%s\t%s\t%s\n' "$state" "$bin" "$f"
        done
        ;;
    *)
        die "usage: $0 grant|revoke|print <binary> [checkout] | installable <binary> | list"
        ;;
esac
