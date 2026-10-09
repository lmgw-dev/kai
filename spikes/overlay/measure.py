#!/usr/bin/env python3
"""usage: measure.py <label> [--cpu SECS] [--warm SECS] [--base SECS] -- <overlay args>
One foreground run of ./target/release/overlay (it exits by itself). After --warm seconds: PSS of the UI process and every
descendant (WebKitWebProcess, WebKitNetworkProcess, ...) from /proc/<pid>/smaps_rollup. Then, for --cpu seconds: CPU ticks of those
processes (/proc/<pid>/stat utime+stime) and nvidia-smi power/utilisation every 500 ms. With --base S a no-overlay GPU window of S seconds
runs first (baseline right before the comparison). Appends one JSON object to results/measure.jsonl."""
import json, os, subprocess, sys, time, statistics, signal
here = os.path.dirname(os.path.abspath(__file__))
a = sys.argv[1:]
label = a[0]; i = a.index("--"); opts = a[1:i]; ov = a[i + 1:]
def opt(n, d):
    return float(opts[opts.index(n) + 1]) if n in opts else d
warm, cpu_s, base_s = opt("--warm", 6), opt("--cpu", 0), opt("--base", 0)
TICK = os.sysconf("SC_CLK_TCK")

def ppid_map():
    m = {}
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                s = open(f"/proc/{p}/stat").read()
                m[int(p)] = (int(s[s.rindex(")") + 2:].split()[1]), s[s.index("(") + 1:s.rindex(")")])
            except OSError:
                pass
    return m
def tree(root):
    m = ppid_map(); out = [root]; k = 0
    while k < len(out):
        out += [p for p, (pp, _) in m.items() if pp == out[k] and p not in out]; k += 1
    return [(p, m[p][1]) for p in out if p in m]
def pss(p):
    try:
        for l in open(f"/proc/{p}/smaps_rollup"):
            if l.startswith("Pss:"): return int(l.split()[1])
    except OSError: return None
def ticks(p):
    try:
        s = open(f"/proc/{p}/stat").read().split(")")[1].split()
        return int(s[11]) + int(s[12])
    except OSError: return None
def gpu_window(secs, pids=()):
    pm = subprocess.Popen(["nvidia-smi", "pmon", "-s", "u", "-d", "1"], stdout=subprocess.PIPE, text=True) if pids else None
    pr = subprocess.Popen(["nvidia-smi", "--query-gpu=power.draw,utilization.gpu", "--format=csv,noheader,nounits", "-lms", "500"],
                          stdout=subprocess.PIPE, text=True)
    time.sleep(secs); pr.terminate(); out = pr.communicate()[0]
    pmon = {}
    if pm:
        pm.terminate(); po = pm.communicate()[0]
        for l in po.splitlines():
            f = l.split()
            if len(f) >= 5 and f[1].isdigit() and int(f[1]) in pids:
                pmon.setdefault(int(f[1]), []).append(f[3] if f[3] != "-" else "0")
    rows = [tuple(float(x) for x in l.split(",")) for l in out.strip().splitlines() if l.strip()]
    rows = rows[1:] if len(rows) > 2 else rows  # first sample is startup
    return {"n": len(rows), "power_w": round(statistics.mean(r[0] for r in rows), 2), "util_pct": round(statistics.mean(r[1] for r in rows), 2),
            "power_max": max(r[0] for r in rows),
            "power_med": statistics.median(r[0] for r in rows), "util_med": statistics.median(r[1] for r in rows),
            "pmon_sm_pct": {str(k): v for k, v in pmon.items()}}
res = {"label": label, "args": ov, "time": time.strftime("%H:%M:%S")}
if base_s:
    res["gpu_base"] = gpu_window(base_s)
env = dict(os.environ)
pr = subprocess.Popen([os.path.join(here, "target/release/overlay")] + ov, stdout=open(os.path.join(here, f"results/{label}.log"), "w"),
                      stderr=subprocess.STDOUT, env=env)
try:
    time.sleep(warm)
    procs = tree(pr.pid)
    res["procs"] = [{"pid": p, "comm": c, "pss_kb": pss(p)} for p, c in procs]
    res["pss_total_kb"] = sum(x["pss_kb"] or 0 for x in res["procs"])
    res["pss_webkit_kb"] = sum(x["pss_kb"] or 0 for x in res["procs"] if x["comm"].startswith("WebKit"))
    if cpu_s:
        t0 = {p: ticks(p) for p, _ in procs}; w0 = time.time()
        res["gpu"] = gpu_window(cpu_s, tuple(p for p, _ in procs))
        w = time.time() - w0
        t1 = {p: ticks(p) for p, _ in procs}
        res["cpu_pct"] = {f"{c}:{p}": round(100 * ((t1[p] or 0) - (t0[p] or 0)) / TICK / w, 2) for p, c in procs if t0[p] is not None}
        res["cpu_pct_total"] = round(sum(res["cpu_pct"].values()), 2)
        res["window_s"] = round(w, 1)
    pr.wait(timeout=max(60, 0) )
finally:
    if pr.poll() is None:
        pr.terminate()
        try: pr.wait(5)
        except Exception: pr.kill()
open(os.path.join(here, "results/measure.jsonl"), "a").write(json.dumps(res) + "\n")
print(json.dumps(res, indent=1))
