#!/usr/bin/env bash
# WP0 owner checks for Kai, a few minutes. Run from a terminal on the Plasma desktop:  ./owner-checks.sh [a b f l m]
# Default: a b f m  (l, the locked wallet, is optional: ./owner-checks.sh l).  One verdict line per point goes to
# results/owner-verdicts.txt (append-only). Temporary files (a .desktop, a mime default) are removed on exit.
# Nothing leaves the machine. Needs the built probe: see BIN below.
set -uo pipefail
cd "$(dirname "$0")"
BIN=../overlay/target/release/wp0
RES=results/owner-verdicts.txt
mkdir -p results
[ -x "$BIN" ] || { echo "Build first:  . ./.cargo-env.sh && CARGO_TARGET_DIR=../overlay/target cargo build --release"; exit 1; }
points=("$@"); [ ${#points[@]} -eq 0 ] && points=(a b f m)
APPS=~/.local/share/applications
TMPFILES=()
MIMEBAK=""
cleanup() {
  for f in "${TMPFILES[@]}"; do rm -f "$f"; done
  if [ -n "$MIMEBAK" ] && [ -f "$MIMEBAK" ]; then cp "$MIMEBAK" ~/.config/mimeapps.list; rm -f "$MIMEBAK"; fi
}
trap cleanup EXIT
say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
ask()  { local a; read -r -p "$1 " a; echo "$a"; }
verdict() { echo "$(date -Is) ($1) $2" | tee -a "$RES"; }
enter() { read -r -p "  [Enter] when ready... " _; }

point_a() {
  say "(a) Real click on the interactive region; the rest of the ring must click through."
  echo "  A 320x320 ring appears at the bottom right for 30 s. A blue button sits at its lower left."
  echo "  1. Put a window under the ring (a terminal or desktop icons) and click on the ring's CENTRE a few times: the click must reach the window below."
  echo "  2. Click the BLUE BUTTON 3 times: it turns amber/blue each time."
  enter
  "$BIN" --mode click --interactive --reapply --scenario speaking --secs 30 > results/owner-a.log 2>&1
  read -r clicks outside < <(python3 - <<'PY'
import re, json
clicks = outside = 0
for l in open("results/owner-a.log"):
    m = re.search(r"\] (\{.*\})$", l)
    if not m: continue
    try: e = json.loads(m.group(1))
    except ValueError: continue
    if e.get("ev") == "click": clicks += 1
    if e.get("ev") in ("pointermove", "pointerdown", "click") and "x" in e:
        if not (10 <= e["x"] <= 130 and 270 <= e["y"] <= 310): outside += 1
print(clicks, outside)
PY
)
  echo "  button clicks received: $clicks; pointer events outside the button rect: $outside (must be 0)"
  local ct; ct=$(ask "  Did your clicks on the ring's non-button area reach the window below? [y/n]")
  if [ "$clicks" -ge 1 ] && [ "$outside" -eq 0 ] && [ "$ct" = y ]; then verdict a "VERIFIED: button clicks=$clicks, outside events=0, click-through confirmed by hand"
  else verdict a "FAILED/PARTIAL: button clicks=$clicks, outside events=$outside, click-through answer=$ct (see results/owner-a.log)"; fi
}

point_b() {
  say "(b, second half) The ring over a fullscreen GAME."
  echo "  The ring will show (bottom right, 55 s, animated). Start any game FULLSCREEN right after pressing Enter (borderless or exclusive)."
  echo "  Look for the ring above the game; if the game shows an FPS counter, note the value with and without the ring (it disappears after 55 s)."
  enter
  "$BIN" --mode ring --layer overlay --scenario speaking --secs 55 > results/owner-b.log 2>&1 &
  local pid=$!
  echo "  ... ring is up for 55 s. Quit the game when you are done (the ring ends by itself)."
  wait $pid
  local vis game fps
  vis=$(ask "  Was the ring visible above the fullscreen game? [y/n/flicker]")
  game=$(ask "  Game and fullscreen mode (exclusive/borderless/windowed):")
  fps=$(ask "  FPS with ring / without (or 'n/a'):")
  verdict b "second half: ring visible over game=$vis; game=$game; fps=$fps"
}

point_f() {
  say "(f) XDG GlobalShortcuts portal: Activated / Deactivated / key repeat."
  echo "  A temporary desktop entry dev.rootshell.kaiwp0test is installed (the portal needs a .desktop file for the app id) and removed afterwards."
  echo "  1. A bind dialog appears: accept a test shortcut (e.g. Ctrl+Shift+F9). 2. Within 25 s press it once briefly, wait 2 s, then hold it 3 s and release."
  enter
  local df=$APPS/dev.rootshell.kaiwp0test.desktop
  printf '[Desktop Entry]\nType=Application\nName=kai wp0 test (temporary)\nExec=true\nNoDisplay=true\n' > "$df"; TMPFILES+=("$df")
  systemd-run --user --scope -q -u app-dev.rootshell.kaiwp0test-wp0.scope python3 ./shortcuts-probe.py --listen 25 2>&1 | tee results/owner-f.log
  grep '^VERDICT f' results/owner-f.log | sed 's/^VERDICT /(/; s/:/)/' | while read -r l; do echo "$(date -Is) $l" >> "$RES"; done
  if ! grep -q 'Deactivated arrives' results/owner-f.log; then
    local r; r=$(ask "  The portal did not deliver releases. Try the KGlobalAccel fallback now? [y/n]")
    if [ "$r" = y ]; then
      python3 ./shortcuts-probe.py --kglobalaccel --listen 25 2>&1 | tee -a results/owner-f.log
      grep '^VERDICT f' results/owner-f.log | tail -1 | sed 's/^VERDICT /(/; s/:/)/' | while read -r l; do echo "$(date -Is) $l" >> "$RES"; done
    fi
  fi
  echo "  Cleanup note: KDE keeps the bound test shortcut under System Settings > Shortcuts > 'kai wp0 test'. Remove it there (or run: kwriteconfig6 --file kglobalshortcutsrc --group dev.rootshell.kaiwp0test --delete-group)."
  local n; n=$(ask "  Free-text remark for (f) (e.g. dialog looked odd), or Enter:")
  [ -n "$n" ] && verdict f "remark: $n"
}

point_l() {
  say "(l) Locked wallet at login (optional)."
  echo "  Best test: log out, log in, and decline/skip the wallet unlock so the Secret Service collection stays locked; then run './owner-checks.sh l'."
  echo "  Quick simulation (this script): it LOCKS your default collection now (other apps must unlock it again; you can unlock via the prompt it raises)."
  local go; go=$(ask "  Lock the wallet collection now and probe? [y/n/already-locked]")
  [ "$go" = n ] && { verdict l "SKIPPED by owner"; return; }
  local SS=org.freedesktop.secrets C=/org/freedesktop/secrets/collection/kdewallet
  [ "$go" = y ] && busctl --user call $SS /org/freedesktop/secrets org.freedesktop.Secret.Service Lock ao 1 $C >/dev/null
  local locked t0 t1 prompt
  locked=$(busctl --user get-property $SS $C org.freedesktop.Secret.Collection Locked | awk '{print $2}')
  t0=$(date +%s%N)
  busctl --user call $SS /org/freedesktop/secrets org.freedesktop.Secret.Service SearchItems a{ss} 1 kai-wp0-test none > results/owner-l-search.txt 2>&1
  t1=$(date +%s%N)
  prompt=$(timeout 5 busctl --user call $SS /org/freedesktop/secrets org.freedesktop.Secret.Service Unlock ao 1 $C 2>&1 | tail -1)
  echo "  Locked=$locked; SearchItems answered in $(( (t1-t0)/1000000 )) ms (no dialog); Unlock reply: $prompt"
  if [ "$locked" = true ]; then
    echo "  Now the unlock prompt: a dialog appears; unlock or cancel it."
    local pp; pp=$(echo "$prompt" | grep -o '"/org/freedesktop/secrets/prompt/[^"]*"' | tr -d '"')
    [ -n "$pp" ] && busctl --user call $SS "$pp" org.freedesktop.Secret.Prompt Prompt s "" >/dev/null 2>&1
    local end=$((SECONDS+40)) st=true
    while [ $SECONDS -lt $end ]; do
      st=$(busctl --user get-property $SS $C org.freedesktop.Secret.Collection Locked | awk '{print $2}'); [ "$st" = false ] && break; sleep 1
    done
    verdict l "VERIFIED: locked state detectable (Locked=true), SearchItems non-blocking ($(( (t1-t0)/1000000 )) ms), Unlock returned a prompt path=${pp:-none}, after the dialog Locked=$st"
  else
    verdict l "Not locked at probe time (Locked=$locked): inconclusive"
  fi
}

point_m() {
  say "(m) lmgw-pair: link click from a browser and from lmgw's own window."
  echo "  A temporary handler for lmgw-pair: is installed and your default is restored afterwards. It logs only the scheme, argument length and cgroup (never the key)."
  local H=/tmp/wp0-pair-handler.sh LOG
  LOG=$PWD/results/owner-m-clicks.log
  cat > "$H" <<HEOF
#!/usr/bin/env bash
echo "\$(date -Is) handler start: scheme=\${1%%:*} arglen=\${#1} cgroup=\$(tail -1 /proc/self/cgroup)" >> "$LOG"
HEOF
  chmod +x "$H"; TMPFILES+=("$H")
  local df=$APPS/dev.rootshell.kaiwp0pair.desktop
  printf '[Desktop Entry]\nType=Application\nName=kai wp0 pair test (temporary)\nExec=%s %%u\nMimeType=x-scheme-handler/lmgw-pair;\nNoDisplay=true\n' "$H" > "$df"; TMPFILES+=("$df")
  MIMEBAK=$(mktemp /tmp/wp0-mimeapps-XXXX); cp ~/.config/mimeapps.list "$MIMEBAK" 2>/dev/null || : > "$MIMEBAK"
  update-desktop-database "$APPS" 2>/dev/null
  xdg-mime default dev.rootshell.kaiwp0pair.desktop x-scheme-handler/lmgw-pair
  : > "$LOG"
  local page=/tmp/wp0-pair.html; TMPFILES+=("$page")
  echo '<!doctype html><meta charset=utf-8><body style="font:18px sans-serif;padding:2em"><p><a href="lmgw-pair:?v=1&url=http%3A%2F%2F127.0.0.1%3A1&name=wp0test&key=TESTKEY-NOT-REAL">Click: lmgw-pair test link</a></p>' > "$page"
  echo "  Browser part: your default browser opens a page; click the link (allow the browser's 'open application' prompt if it asks)."
  enter
  xdg-open "file://$page"
  local end=$((SECONDS+45)); while [ $SECONDS -lt $end ] && [ ! -s "$LOG" ]; do sleep 1; done
  local b; b=$(wc -l < "$LOG")
  echo "  handler runs after browser click: $b"
  local bb; bb=$(ask "  Which browser was it, and did it ask first? (free text)")
  echo
  echo "  lmgw's own window: if lmgw's dashboard has a lmgw-pair link or button (Devices card), click it now. Otherwise answer n/a."
  local w; w=$(ask "  Click it, then answer: [clicked/n/a]")
  sleep 2
  local total; total=$(wc -l < "$LOG")
  verdict m "browser click -> handler runs=$b ($bb); lmgw own window=$w -> total handler runs=$total; handler cgroups in results/owner-m-clicks.log"
  echo "  Note: WebKitGTK alone never launches an external handler (wp0 --mode nav: decide-policy, then load-failed 'The URL can't be shown'); lmgw's window must hand the link to the system."
  echo "  Restoring your mime default..."
}

for p in "${points[@]}"; do "point_$p"; done
say "Done. Verdicts: $RES"
