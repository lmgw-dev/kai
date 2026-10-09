#!/usr/bin/env python3
# A small MCP Apps server for WP12's live check (scripts/live-check-widgets.sh, section apps): a
# stdio MCP server (newline-delimited JSON-RPC, no dependencies) registered on the dev lmgw with a
# tool prefix, whose view Kai shows with overlay_app. It offers:
#   - card_show    (model and app; read-only) whose _meta.ui.resourceUri is ui://fixture/card;
#   - card_data    (app only; read-only) returning structuredContent {"value": "fixture-<n>"};
#   - card_bump    (app only; changes state, destructiveHint false) counting its calls: Kai asks
#     the owner first unless the app is trusted;
#   - card_reset   (app only; changes state, no destructiveHint: destructive by MCP's default):
#     Kai asks the owner even for a trusted app;
#   - model_only   (model only): a view's call is refused by Kai;
#   - card_live    (model and app; read-only) whose view, ui://fixture/live, is written like an
#     SDK-built one: it shows nothing until the host sends the call's
#     ui/notifications/tool-input and tool-result (or tool-cancelled), which Kai does for a call
#     in its own conversation; it reports what it saw so far at each of them (and once its own
#     calls are done): the order, the toolInfo it was given and two state-changing calls of its
#     own (card_bump, card_reset);
#   - ui://fixture/card: the view, whose _meta.ui asks for a network origin and the camera (Kai
#     refuses both and names them), and ui://fixture/extra, plain text the view reads; the view
#     lists both through Kai (resources/list) and answers Kai's ui/resource-teardown.
# The view does the MCP Apps handshake over postMessage, then each of its checks, shows the data
# in large text and reports what it saw by navigating to kai-sandbox://widget/report?<json>
# (refused by Kai's sandbox, and seen by the live check).
# usage: python3 -I scripts/mcp-apps-fixture.py   (lmgw starts it; it reads stdin, writes stdout)
import json
import sys

VIEW = r"""<!doctype html>
<meta charset="utf-8">
<style>
  html, body { margin: 0; height: 100%; background: #232629; color: #eff0f1;
               font: 28px/1.3 "Noto Sans", sans-serif; }
  #v { box-sizing: border-box; height: 100%; padding: 24px; border: 4px solid #3daee9; }
</style>
<div id="v">fixture: waiting</div>
<script>
  const pending = new Map();
  let n = 0;
  const notes = [];
  const rpc = (method, params) => new Promise((done) => {
    const id = ++n;
    pending.set(id, done);
    window.parent.postMessage({ jsonrpc: "2.0", id, method, params }, "*");
  });
  window.addEventListener("message", (e) => {
    const m = e.data;
    if (m && m.method === "ui/resource-teardown") {
      // Kai's request before the view goes: answered at once.
      window.parent.postMessage({ jsonrpc: "2.0", id: m.id, result: {} }, "*");
    } else if (m && m.id !== undefined && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    } else if (m && m.method) {
      notes.push(m.method);
    }
  });
  const err = (a) => (a.error ? a.error.code + " " + a.error.message : "ok");
  (async () => {
    const r = {};
    const init = await rpc("ui/initialize", {
      protocolVersion: "2026-01-26",
      appInfo: { name: "fixture-view", version: "1" },
      appCapabilities: {},
    });
    r.init = init.result ? init.result.protocolVersion : err(init);
    r.serverTools = !!(init.result && init.result.hostCapabilities.serverTools);
    window.parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/initialized", params: {} }, "*");
    const d = await rpc("tools/call", { name: "card_data", arguments: {} });
    r.data = d.result ? d.result.structuredContent.value : err(d);
    document.getElementById("v").textContent = "fixture: " + r.data;
    const b = await rpc("tools/call", { name: "card_bump", arguments: { by: 1 } });
    r.bump = b.result ? "ran" : err(b);
    const mo = await rpc("tools/call", { name: "model_only", arguments: {} });
    r.modelOnly = mo.result ? "ran" : err(mo);
    const o = await rpc("tools/call", { name: "kailive__live_echo", arguments: { text: "x" } });
    r.otherServer = o.result ? "ran" : err(o);
    const x = await rpc("resources/read", { uri: "ui://fixture/extra" });
    r.read = x.result ? x.result.contents[0].text : err(x);
    r.readUri = x.result ? x.result.contents[0].uri : "";
    const rl = await rpc("resources/list", {});
    r.list = rl.result ? rl.result.resources.map((x) => x.uri).sort().join(",") : err(rl);
    const u = await rpc("resources/read", { uri: "urn:fixture:x" });
    r.readUrn = u.result ? "ran" : err(u);
    const l = await rpc("ui/open-link", { url: "https://example.com/" });
    r.openLink = l.result ? "ran" : err(l);
    const p = await rpc("ping", {});
    r.ping = p.result ? "ok" : err(p);
    r.notes = notes;
    location.href = "kai-sandbox://widget/report?" + encodeURIComponent(JSON.stringify(r));
  })();
</script>
"""

LIVE = r"""<!doctype html>
<meta charset="utf-8">
<style>
  html, body { margin: 0; height: 100%; background: #232629; color: #eff0f1;
               font: 28px/1.3 "Noto Sans", sans-serif; }
  #v { box-sizing: border-box; height: 100%; padding: 24px; border: 4px solid #3daee9; }
</style>
<div id="v">live view: waiting for the call</div>
<script>
  const pending = new Map();
  let n = 0;
  const seen = { order: [] };
  const v = document.getElementById("v");
  const rpc = (method, params) => new Promise((done) => {
    const id = ++n;
    pending.set(id, done);
    window.parent.postMessage({ jsonrpc: "2.0", id, method, params }, "*");
  });
  const err = (a) => (a.error ? a.error.code + " " + a.error.message : "ok");
  // Every report is what the view saw so far (a refused navigation leaves the page as it is).
  const report = () => {
    seen.step = (seen.step || 0) + 1;
    location.href = "kai-sandbox://widget/report?" + encodeURIComponent(JSON.stringify(seen));
  };
  window.addEventListener("message", (e) => {
    const m = e.data;
    if (!m || typeof m !== "object") return;
    if (m.method === "ui/resource-teardown") {
      window.parent.postMessage({ jsonrpc: "2.0", id: m.id, result: {} }, "*");
    } else if (m.id !== undefined && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    } else if (m.method === "ui/notifications/tool-input") {
      seen.order.push("input");
      seen.input = m.params.arguments;
      v.textContent = "live view: " + (m.params.arguments.city || "?") + ", the tool runs";
      report();
    } else if (m.method === "ui/notifications/tool-result") {
      seen.order.push("result");
      seen.result = m.params.structuredContent;
      seen.text = m.params.content.map((c) => c.text).join("");
      seen.types = m.params.content.map((c) => c.type);
      seen.isError = m.params.isError;
      v.textContent = "live view: " + seen.result.city + " " + seen.result.temp + " °C";
      report();
    } else if (m.method === "ui/notifications/tool-cancelled") {
      seen.order.push("cancelled");
      seen.cancelled = m.params.reason;
      v.textContent = "live view: cancelled (" + m.params.reason + ")";
      report();
    }
  });
  (async () => {
    const init = await rpc("ui/initialize", {
      protocolVersion: "2026-01-26",
      appInfo: { name: "fixture-live", version: "1" },
      appCapabilities: {},
    });
    const info = init.result && init.result.hostContext.toolInfo;
    seen.toolName = info ? info.tool.name : null;
    seen.toolUri = info ? info.tool._meta.ui.resourceUri : null;
    seen.toolId = info && info.id !== undefined ? info.id : null;
    window.parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/initialized", params: {} }, "*");
    const b = await rpc("tools/call", { name: "card_bump", arguments: { by: 1 } });
    seen.bump = b.result ? "ran" : err(b);
    const z = await rpc("tools/call", { name: "card_reset", arguments: {} });
    seen.reset = z.result ? "ran" : err(z);
    report();
  })();
</script>
"""

TOOLS = [
    {
        "name": "card_show",
        "description": "Show the fixture's card.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
        "_meta": {"ui": {"resourceUri": "ui://fixture/card"}},
    },
    {
        "name": "card_data",
        "description": "The card's data.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
        "_meta": {"ui": {"resourceUri": "ui://fixture/card", "visibility": ["app"]}},
    },
    {
        "name": "card_bump",
        "description": "Count one more.",
        "inputSchema": {"type": "object", "properties": {"by": {"type": "integer"}}},
        "annotations": {"readOnlyHint": False, "destructiveHint": False},
        "_meta": {"ui": {"visibility": ["app"]}},
    },
    {
        "name": "card_reset",
        "description": "Start counting again.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": False},
        "_meta": {"ui": {"visibility": ["app"]}},
    },
    {
        "name": "card_live",
        "description": "Show the live card for a city.",
        "inputSchema": {"type": "object", "properties": {"city": {"type": "string"}}},
        "annotations": {"readOnlyHint": True},
        "_meta": {"ui": {"resourceUri": "ui://fixture/live"}},
    },
    {
        "name": "model_only",
        "description": "Only for the model.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
        "_meta": {"ui": {"visibility": ["model"]}},
    },
]

RESOURCES = [
    {"uri": "ui://fixture/card", "name": "card", "mimeType": "text/html;profile=mcp-app"},
    {"uri": "ui://fixture/extra", "name": "extra", "mimeType": "text/plain"},
    {"uri": "ui://fixture/live", "name": "live", "mimeType": "text/html;profile=mcp-app"},
]

state = {"calls": 0, "bumps": 0}


def result(req, value):
    return {"jsonrpc": "2.0", "id": req["id"], "result": value}


def error(req, code, message):
    return {"jsonrpc": "2.0", "id": req["id"], "error": {"code": code, "message": message}}


def handle(req):
    method = req.get("method")
    params = req.get("params") or {}
    if "id" not in req:
        return None
    if method == "initialize":
        return result(req, {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "kai-apps-fixture", "version": "1"},
        })
    if method == "ping":
        return result(req, {})
    if method == "tools/list":
        return result(req, {"tools": TOOLS})
    if method == "tools/call":
        name = params.get("name")
        if name in ("card_show", "card_data"):
            state["calls"] += 1
            value = f"fixture-{state['calls']}"
            return result(req, {
                "content": [{"type": "text", "text": value}],
                "structuredContent": {"value": value},
            })
        if name == "card_bump":
            state["bumps"] += 1
            return result(req, {"content": [{"type": "text", "text": f"bumps: {state['bumps']}"}]})
        if name == "card_reset":
            state["bumps"] = 0
            return result(req, {"content": [{"type": "text", "text": "bumps: 0"}]})
        if name == "card_live":
            city = (params.get("arguments") or {}).get("city", "?")
            return result(req, {
                "content": [{"type": "text", "text": f"{city}: 21"}],
                "structuredContent": {"city": city, "temp": 21},
            })
        if name == "model_only":
            return result(req, {"content": [{"type": "text", "text": "model"}]})
        return error(req, -32602, f"no tool {name}")
    if method == "resources/list":
        return result(req, {"resources": RESOURCES})
    if method == "resources/templates/list":
        return result(req, {"resourceTemplates": []})
    if method == "resources/read":
        uri = params.get("uri")
        if uri == "ui://fixture/card":
            return result(req, {"contents": [{
                "uri": uri,
                "mimeType": "text/html;profile=mcp-app",
                "text": VIEW,
                "_meta": {"ui": {
                    "csp": {"connectDomains": ["https://api.example.com"]},
                    "permissions": {"camera": {}},
                    "prefersBorder": False,
                }},
            }]})
        if uri == "ui://fixture/live":
            return result(req, {"contents": [{
                "uri": uri, "mimeType": "text/html;profile=mcp-app", "text": LIVE,
            }]})
        if uri == "ui://fixture/extra":
            return result(req, {"contents": [{"uri": uri, "mimeType": "text/plain",
                                              "text": "extra-text"}]})
        return error(req, -32002, f"no resource {uri}")
    return error(req, -32601, f"no method {method}")


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except ValueError:
        continue
    out = handle(req)
    if out is not None:
        sys.stdout.write(json.dumps(out) + "\n")
        sys.stdout.flush()
