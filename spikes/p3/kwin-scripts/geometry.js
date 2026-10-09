// Moves and resizes one window by its internal id: ARGS.id, ARGS.frame {x, y, width, height}.
try {
    const w = byId(ARGS.id);
    if (!w) throw new Error("no window " + ARGS.id);
    const before = rect(w.frameGeometry);
    w.frameGeometry = ARGS.frame;
    reply({ ok: true, before: before, after: rect(w.frameGeometry) });
} catch (e) { reply({ ok: false, error: String(e) }); }
