#!/usr/bin/env bash
# WP13 (g): Baloo's query path without KRunner. A private Baloo (its own HOME, config, index and
# bus inside the nested session) indexes a few test files; then the candidate paths for
# `find_files` are timed: baloosearch6 (the CLI, reading the index directly), the baloo runner's
# D-Bus interface (org.kde.krunner1 on org.kde.runners.baloo, no KRunner process), and whether
# either needs the indexer running. The session's own index is never opened.
# usage: spikes/p3/run-g.sh     (output: spikes/p3/.run/g.log)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)

if [[ ${1:-} == --inside ]]; then
    set +e
    say() { echo; echo "## $*"; }
    ms() { echo $(( ($(date +%s%N) - $1) / 1000000 )); }
    docs=$HOME/docs
    mkdir -p "$docs/sub" "$XDG_CONFIG_HOME"
    printf 'Quarterly numbers. The zebra budget for Kai is fine.\n' > "$docs/quarterly-report.txt"
    printf '# Notes\nzebra crossing at the corner\n' > "$docs/notes.md"
    printf 'nothing to see here\n' > "$docs/sub/other.txt"
    printf 'a zebra in a subfolder\n' > "$docs/sub/deep-zebra-file.txt"
    cat > "$XDG_CONFIG_HOME/baloofilerc" <<EOF
[Basic Settings]
Indexing-Enabled=true

[General]
folders[\$e]=$docs/
exclude folders[\$e]=
only basic indexing=false
index hidden folders=false
EOF
    say "the private indexer"
    t0=$(date +%s%N)
    /usr/libexec/kf6/baloo_file > "$P3_DIR/baloo_file.log" 2>&1 &
    bpid=$!
    for i in $(seq 120); do
        st=$(balooctl6 status 2>&1)
        grep -q 'Total files indexed: 6' <<< "$st" && grep -q 'waiting for content indexing: 0' <<< "$st" \
            && grep -q 'state: Idle' <<< "$st" && break
        sleep 0.5
    done
    echo "indexed after $(ms "$t0") ms:"; balooctl6 status 2>&1 | sed 's/^/  /'
    echo "index files:"; find "$XDG_DATA_HOME/baloo" -type f -printf '  %P %s bytes\n'
    say "baloo_file's D-Bus surface (is there a query method?)"
    for p in /fileindexer /scheduler /; do
        busctl --user introspect org.kde.baloo "$p" 2>/dev/null | grep -E '^org\.kde|^\.' | grep -v 'org.freedesktop' | sed 's/^/  /'
    done

    say "path 1: baloosearch6 (content word, indexer running)"
    t0=$(date +%s%N); baloosearch6 zebra; echo "  $(ms "$t0") ms (process start included)"
    say "path 1: baloosearch6 -d <dir> -l 1 zebra; filename: query; -t Document"
    baloosearch6 -d "$docs/sub" zebra
    baloosearch6 -l 1 zebra
    baloosearch6 "filename:quarterly*"
    baloosearch6 -t Document zebra
    say "path 2: the baloo runner over D-Bus (org.kde.krunner1, no KRunner)"
    /usr/libexec/baloorunner > "$P3_DIR/baloorunner.log" 2>&1 &
    rpid=$!
    for i in $(seq 50); do busctl --user status org.kde.runners.baloo > /dev/null 2>&1 && break; sleep 0.1; done
    busctl --user introspect org.kde.runners.baloo /runner | grep -E '^\.' | sed 's/^/  /'
    for q in zebra quarterly "deep zebra"; do
        t0=$(date +%s%N)
        out=$(busctl --user --json=short call org.kde.runners.baloo /runner org.kde.krunner1 Match s "$q")
        echo "  Match(\"$q\") $(ms "$t0") ms: $out" | cut -c1-700
    done
    t0=$(date +%s%N); busctl --user call org.kde.runners.baloo /runner org.kde.krunner1 Match s zebra > /dev/null
    echo "  warm Match(zebra) $(ms "$t0") ms"
    busctl --user call org.kde.runners.baloo /runner org.kde.krunner1 Actions | cut -c1-300 | sed 's/^/  Actions: /'

    say "a new file shows up (inotify through the running indexer)"
    printf 'zebra arrives late\n' > "$docs/late-zebra.txt"
    t0=$(date +%s%N)
    for i in $(seq 60); do baloosearch6 zebra | grep -q late-zebra && break; sleep 0.5; done
    echo "  late-zebra.txt found by baloosearch6 after $(ms "$t0") ms: $(baloosearch6 zebra | grep -c late-zebra) hit"

    say "result caps: 25 more matching files"
    for i in $(seq 25); do printf 'zebra number %d\n' "$i" > "$docs/sub/batch-$i.txt"; done
    t0=$(date +%s%N)
    for i in $(seq 120); do [[ $(baloosearch6 -l 1000 zebra | grep -c batch-) == 25 ]] && break; sleep 0.5; done
    echo "  25 new files searchable after $(ms "$t0") ms"
    echo "  baloosearch6 zebra (no -l): $(baloosearch6 zebra | grep -c '^/') results"
    echo "  baloosearch6 -l 1000 zebra: $(baloosearch6 -l 1000 zebra | grep -c '^/') results"
    out=$(busctl --user --json=short call org.kde.runners.baloo /runner org.kde.krunner1 Match s zebra)
    echo "  runner Match(zebra): $(python3 -c 'import json,sys; print(len(json.loads(sys.argv[1])["data"][0]))' "$out") results"
    t0=$(date +%s%N); baloosearch6 -l 1000 zebra > /dev/null; echo "  baloosearch6 -l 1000: $(ms "$t0") ms"

    say "with the indexer stopped"
    balooctl6 suspend > /dev/null 2>&1
    kill "$bpid"; wait "$bpid" 2>/dev/null
    busctl --user status org.kde.baloo > /dev/null 2>&1 && echo "  org.kde.baloo still on the bus" || echo "  org.kde.baloo gone"
    t0=$(date +%s%N); baloosearch6 zebra | sed 's/^/  /'; echo "  baloosearch6 $(ms "$t0") ms"
    out=$(busctl --user --json=short call org.kde.runners.baloo /runner org.kde.krunner1 Match s zebra)
    echo "  runner Match(zebra): $(grep -o 'file://[^"]*' <<< "$out" | tr '\n' ' ')$(grep -o '"[^"]*\.txt"' <<< "$out" | head -0)"
    echo "  runner raw: ${out:0:300}"
    kill "$rpid"; wait "$rpid" 2>/dev/null
    say "Indexing-Enabled=false: what baloosearch6 says"
    sed -i 's/Indexing-Enabled=true/Indexing-Enabled=false/' "$XDG_CONFIG_HOME/baloofilerc"
    baloosearch6 zebra 2>&1 | sed 's/^/  /'
    exit 0
fi

"$here/nested.sh" g -- "$0" --inside 2>&1 | tee "$here/.run/g.log"
