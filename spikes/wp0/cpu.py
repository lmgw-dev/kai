#!/usr/bin/env python3
"""usage: cpu.py <label> <warm_s> <window_s> -- <wp0 args>
Runs ./target wp0 (it exits by itself), waits warm_s, then reports CPU% (of one core) of the UI process and each
WebKit child over window_s from /proc/<pid>/stat utime+stime, plus PSS. Appends one JSON line to results/cpu.jsonl."""
import json, os, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("WP0_BIN", os.path.join(here, "..", "overlay", "target", "release", "wp0"))
label, warm, win = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]); args = sys.argv[sys.argv.index("--") + 1:]
TICK = os.sysconf("SC_CLK_TCK")
def kids(root):
    m = {}
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                s = open(f"/proc/{p}/stat").read(); m[int(p)] = (int(s[s.rindex(")") + 2:].split()[1]), s[s.index("(") + 1:s.rindex(")")])
            except OSError: pass
    out, k = [root], 0
    while k < len(out):
        out += [p for p, (pp, _) in m.items() if pp == out[k] and p not in out]; k += 1
    return {p: m[p][1] for p in out if p in m}
def ticks(p):
    try: s = open(f"/proc/{p}/stat").read().split(")")[1].split(); return int(s[11]) + int(s[12])
    except OSError: return None
def pss(p):
    try:
        for l in open(f"/proc/{p}/smaps_rollup"):
            if l.startswith("Pss:"): return int(l.split()[1]) // 1024
    except OSError: return None
pr = subprocess.Popen([BIN] + args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
time.sleep(warm)
procs = kids(pr.pid); t0 = {p: ticks(p) for p in procs}; w0 = time.time()
time.sleep(win)
w1 = time.time(); procs2 = kids(pr.pid)
res = {"label": label, "args": args, "window_s": round(w1 - w0, 2), "cpu_pct": {}, "pss_mb": {}}
for p, n in procs.items():
    t1 = ticks(p)
    if t1 is None or t0[p] is None: continue
    res["cpu_pct"][f"{n}"] = round((t1 - t0[p]) / TICK / (w1 - w0) * 100, 2)
    res["pss_mb"][f"{n}"] = pss(p)
res["cpu_total"] = round(sum(res["cpu_pct"].values()), 2)
out = pr.communicate(timeout=60)[0]
for l in out.splitlines():
    if l.startswith("stats"): res["stats"] = l[6:]
print(json.dumps(res))
open(os.path.join(here, "results", "cpu.jsonl"), "a").write(json.dumps(res) + "\n")
