#!/usr/bin/env bash
# The dev grants' files (scripts/dev-grant.sh, §16 Q21) in a scratch HOME and XDG_DATA_DIRS:
# nothing is written to the owner's applications directory and no grant is created for real. Two
# fake builds in two fake checkouts get their own files naming their canonical paths (a symlinked
# path resolves, and names the same file) with the packaged .desktop's two restricted-interface
# keys, no MimeType and NoDisplay; list shows both, and one whose binary is gone as stale; revoke
# removes only its own build's file. One entry per binary: grant refuses beside a stripped
# install-dev entry for the same binary, list marks a grant such an entry shadows, installable
# (install-dev's guard) refuses beside a grant. Exec is quoted as the Desktop Entry spec wants
# (a path with a space), and a path KWin could not match once escaped is refused.
# usage: scripts/dev-grant-check.sh   (exit 1 on a failed check)
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
grant=$here/scripts/dev-grant.sh
scratch=$(mktemp -d "${TMPDIR:-/tmp}/kai-dev-grant-check.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export HOME=$scratch/home XDG_DATA_HOME=$scratch/home/.local/share XDG_DATA_DIRS=$scratch/share
apps=$XDG_DATA_HOME/applications
mkdir -p "$HOME" "$XDG_DATA_DIRS/applications"
failed=0
check() { # <ok?> <what>
    if [[ $1 == 0 ]]; then echo "ok   $2"; else echo "FAIL $2"; failed=1; fi
}
build() { # <checkout>: a fake build's binary
    mkdir -p "$1/target/debug"
    printf '#!/bin/sh\n' > "$1/target/debug/kai"
    chmod +x "$1/target/debug/kai"
}
stripped() { # <file> <Exec value>: an entry as install-dev writes it, without the keys
    printf '[Desktop Entry]\nType=Application\nName=Kai\nExec=%s\nMimeType=x-scheme-handler/lmgw-pair;\n' \
        "$2" > "$1"
}

for w in kai kai-feature; do build "$scratch/$w"; done
ln -s "$scratch/kai-feature" "$scratch/linked"

"$grant" grant "$scratch/kai/target/debug/kai" "$scratch/kai" > /dev/null
"$grant" grant "$scratch/linked/target/debug/kai" "$scratch/linked" > /dev/null
mapfile -t files < <(ls "$apps")
check $(( ${#files[@]} == 2 ? 0 : 1 )) "one file per build (${files[*]})"
main_file=$(ls "$apps" | grep -- '-devgrant-kai-[0-9a-f]\{8\}\.desktop$' || true)
feature_file=$(ls "$apps" | grep -- '-devgrant-kai-feature-[0-9a-f]\{8\}\.desktop$' || true)
check $([[ -n $main_file && -n $feature_file ]]; echo $?) \
    "each file's name says its canonical checkout ($main_file, $feature_file)"
check $([[ $("$grant" print "$scratch/linked/target/debug/kai" "$scratch/linked" | head -n1) \
    == "$("$grant" print "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" | head -n1)" ]]
    echo $?) "the symlinked and the real checkout name the same file"
f=$apps/$feature_file
check $(grep -qx "Exec=\"$scratch/kai-feature/target/debug/kai\"" "$f"; echo $?) \
    "Exec is the canonical absolute path, quoted (the symlinked checkout resolved)"
for key in X-KDE-Wayland-Interfaces X-KDE-DBUS-Restricted-Interfaces; do
    want=$(grep "^$key=" "$here/packaging/dev.rootshell.kai.desktop.in")
    check $(grep -qxF "$want" "$f"; echo $?) "$key as the packaged .desktop has it"
done
check $(grep -qx 'NoDisplay=true' "$f"; echo $?) "NoDisplay=true: no menu entry"
check $( ! grep -q '^MimeType=' "$f"; echo $?) "no MimeType: not a scheme handler"
check $(grep -qx '\[Desktop Entry\]' "$f" && grep -qx 'Type=Application' "$f"; echo $?) \
    "a desktop entry of type Application"
check $([[ $(stat -c %a "$f") == 644 ]]; echo $?) "mode 644"

rm "$scratch/kai-feature/target/debug/kai"
listed=$("$grant" list)
check $(grep -q "^ok	$scratch/kai/target/debug/kai	" <<< "$listed"; echo $?) "list: the present build is ok"
check $(grep -q "^STALE.*	$scratch/kai-feature/target/debug/kai	" <<< "$listed"; echo $?) \
    "list: the build whose binary is gone is stale"
check $([[ -e $f ]]; echo $?) "list removes nothing"

"$grant" revoke "$scratch/linked/target/debug/kai" "$scratch/linked" > /dev/null
check $([[ ! -e $f && -e $apps/$main_file ]]; echo $?) \
    "revoke through the symlinked checkout removes its own build's file only"
again=$("$grant" revoke "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature")
check $(grep -q '^no grant for' <<< "$again"; echo $?) "a second revoke says there is none"
check $( ! "$grant" grant "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" \
    > /dev/null 2>&1; echo $?) "no grant for a binary that does not exist"
printed=$("$grant" print "$scratch/kai/target/debug/kai" "$scratch/kai")
check $([[ $(head -n1 <<< "$printed") == "$main_file" ]]; echo $?) "print names the same file"
check $([[ $(ls "$apps" | wc -l) == 1 ]]; echo $?) "print writes nothing"

# A file of this binary's grant under another name (written through a symlinked checkout before
# the names were canonical): revoke finds it by its marker, grant replaces it.
build "$scratch/kai-feature"
"$grant" print "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" | tail -n +2 \
    > "$apps/dev.rootshell.kai-devgrant-linked-00000000.desktop"
"$grant" grant "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" > /dev/null
check $([[ ! -e $apps/dev.rootshell.kai-devgrant-linked-00000000.desktop && -e $f ]]; echo $?) \
    "grant replaces the same binary's grant under another name"
cp "$f" "$apps/dev.rootshell.kai-devgrant-linked-00000000.desktop"
"$grant" revoke "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" > /dev/null
check $([[ ! -e $apps/dev.rootshell.kai-devgrant-linked-00000000.desktop && ! -e $f ]]; echo $?) \
    "revoke removes the binary's grant files by their marker, whatever their name"

# One entry per binary. KWin takes the first entry its service cache lists for the binary, so a
# stripped entry beside a grant would make the grant chance.
rm "$apps/$main_file"
stripped "$apps/dev.rootshell.kai.desktop" "$scratch/linked/target/debug/kai %u"
out=$("$grant" grant "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 && ! -e $f ]]; echo $?) \
    "grant refuses beside a stripped entry for the same binary (through a symlink)"
check $(grep -qF "$apps/dev.rootshell.kai.desktop" <<< "$out" && grep -q 'uninstall-dev' <<< "$out"
    echo $?) "the refusal names the entry and how to resolve it"
"$grant" grant "$scratch/kai/target/debug/kai" "$scratch/kai" > /dev/null
check $([[ -e $apps/$main_file ]]; echo $?) "another build's stripped entry does not refuse a grant"
mv "$apps/dev.rootshell.kai.desktop" "$XDG_DATA_DIRS/applications/"
check $( ! "$grant" grant "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" \
    > /dev/null 2>&1; echo $?) "a stripped entry in XDG_DATA_DIRS refuses too"
printf 'Hidden=true\n' >> "$XDG_DATA_DIRS/applications/dev.rootshell.kai.desktop"
check $("$grant" grant "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" \
    > /dev/null 2>&1; echo $?) "a Hidden entry does not refuse (KService skips it)"
rm "$XDG_DATA_DIRS/applications/dev.rootshell.kai.desktop"
stripped "$apps/dev.rootshell.kai.desktop" "\"$scratch/kai-feature/target/debug/kai\" %u"
listed=$("$grant" list)
check $(grep -q "^SHADOWED by $apps/dev.rootshell.kai.desktop .*	$scratch/kai-feature/target/debug/kai	" \
    <<< "$listed"; echo $?) "list marks a grant a stripped entry for its binary shadows"
check $(grep -q "^ok	$scratch/kai/target/debug/kai	" <<< "$listed"; echo $?) \
    "list: the other build's grant stays ok"
out=$("$grant" installable "$scratch/linked/target/debug/kai" 2>&1) && rc=0 || rc=$?
check $([[ $rc != 0 ]] && grep -qF "$f" <<< "$out" && grep -q 'revoke-dev' <<< "$out"; echo $?) \
    "installable (install-dev's guard) refuses while a grant names the binary, naming it"
"$grant" revoke "$scratch/kai-feature/target/debug/kai" "$scratch/kai-feature" > /dev/null
check $("$grant" installable "$scratch/kai-feature/target/debug/kai"; echo $?) \
    "installable once the grant is revoked"
rm "$apps/dev.rootshell.kai.desktop"

# Exec as the Desktop Entry spec wants it, and as KWin reads it.
build "$scratch/kai dev"
"$grant" grant "$scratch/kai dev/target/debug/kai" "$scratch/kai dev" > /dev/null
spaced=$(ls "$apps" | grep -- '-devgrant-kai_dev-' || true)
check $([[ -n $spaced ]] && grep -qx "Exec=\"$scratch/kai dev/target/debug/kai\"" "$apps/$spaced"; echo $?) \
    "a path with a space: Exec quoted, the file's name sanitised ($spaced)"
if command -v desktop-file-validate > /dev/null; then
    check $(desktop-file-validate "$apps/$spaced" > /dev/null; echo $?) \
        "desktop-file-validate accepts the quoted Exec"
fi
stripped "$apps/dev.rootshell.kai.desktop" "\"$scratch/kai dev/target/debug/kai\" %u"
check $(grep -q '^SHADOWED' <<< "$("$grant" list)"; echo $?) \
    "a quoted Exec with a space is read as KWin reads it (QProcess::splitCommand)"
rm "$apps/dev.rootshell.kai.desktop"
for odd in 'kai%x' 'kai$x' 'kai"x' 'kai\x' 'kai`x' $'kai\nx'; do
    build "$scratch/$odd"
    before=$(ls -A "$apps" | wc -l)
    out=$("$grant" grant "$scratch/$odd/target/debug/kai" "$scratch/$odd" 2>&1) && rc=0 || rc=$?
    check $([[ $rc != 0 && $(ls -A "$apps" | wc -l) == "$before" ]] \
        && grep -q 'the path contains' <<< "$out"; echo $?) \
        "refused, nothing written: a path containing $(printf %q "${odd:3:1}")"
done
exit "$failed"
