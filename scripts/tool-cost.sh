#!/usr/bin/env bash
# PD6, K22: the context cost of Kai's whole hosted tool list, as lmgw sends it to a model: the list
# (`kai --tool-list <label>`, the debug build with this configuration's [policy]) through lmgw's
# `POST /v1/count_tokens` for each alias the Kai thread uses. A figure to watch, not a cap.
# Usage: scripts/tool-cost.sh <label> [alias…]
#   LMGW_URL  the gateway (default: lmgw.url from Kai's configuration)
#   LMGW_KEY  a key of that gateway holding Inference (and Chat, to read the folder); sent as a
#             header from a pipe, never in curl's argv, and never printed
#   With no alias: the model of Kai's conversation folder (conversation.folder_id, else the
#   ongoing folder named conversation.folder_name with the lowest id).
set -euo pipefail
cd "$(dirname "$0")/.."

label=${1:?usage: scripts/tool-cost.sh <label> [alias…]}
shift
cfg=${XDG_CONFIG_HOME:-$HOME/.config}/kai/config.toml
setting() { # <table> <key>
  [ -f "$cfg" ] || return 0
  python3 -I -c 'import sys, tomllib
v = tomllib.load(open(sys.argv[1], "rb")).get(sys.argv[2], {}).get(sys.argv[3])
print("" if v is None else v)' "$cfg" "$1" "$2"
}
url=${LMGW_URL:-$(setting lmgw url)}
[ -n "$url" ] || { echo "no gateway: set LMGW_URL, or pair Kai" >&2; exit 2; }
[ -n "${LMGW_KEY:-}" ] || { echo "LMGW_KEY: a key of $url that holds Inference" >&2; exit 2; }
out=target/tool-cost
mkdir -p "$out"
auth() { printf 'Authorization: Bearer %s\n' "$LMGW_KEY"; }

cargo build --quiet
target/debug/kai --tool-list "$label" >"$out/tools.json"
tools=$(python3 -I -c 'import json, sys; print(len(json.load(open(sys.argv[1]))))' "$out/tools.json")

aliases=("$@")
if [ ${#aliases[@]} -eq 0 ]; then
  auth | curl -sS --fail-with-body --max-time 30 -H @- "$url/chat/api/folders" >"$out/folders.json" \
    || { echo "cannot read the folders (name the aliases instead): $(cat "$out/folders.json")" >&2; exit 1; }
  alias_=$(python3 -I -c 'import json, sys
folders = json.load(open(sys.argv[1])).get("folders", [])
fid, name = sys.argv[2], sys.argv[3] or "Kai"
pick = [f for f in folders if str(f.get("id")) == fid] or sorted(
    (f for f in folders if f.get("name", "").strip() == name.strip() and f.get("ongoing")),
    key=lambda f: f["id"])
print(((pick[0].get("defaults") or {}).get("model_alias") or "") if pick else "")' \
    "$out/folders.json" "$(setting conversation folder_id)" "$(setting conversation folder_name)")
  [ -n "$alias_" ] || { echo "Kai's folder names no model: name the aliases" >&2; exit 1; }
  aliases=("$alias_")
fi

for a in "${aliases[@]}"; do
  python3 -I -c 'import json, sys
print(json.dumps({"model": sys.argv[1], "input": open(sys.argv[2]).read()}))' "$a" "$out/tools.json" >"$out/body.json"
  answer=$(auth | curl -sS --max-time 120 -H @- -H 'Content-Type: application/json' \
    --data-binary @"$out/body.json" "$url/v1/count_tokens")
  tokens=$(printf '%s' "$answer" | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("tokens", ""))' 2>/dev/null || true)
  if [ -n "$tokens" ]; then
    echo "$a: $tokens tokens for $tools tools under '$label'"
  else
    echo "$a: lmgw answered $answer" >&2
  fi
done
rm -f "$out/body.json" "$out/folders.json"
