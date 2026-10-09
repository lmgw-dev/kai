// Sets one property of one window by its internal id: ARGS.id, ARGS.prop, ARGS.value.
try {
    const w = byId(ARGS.id);
    if (!w) throw new Error("no window " + ARGS.id);
    const before = w[ARGS.prop];
    w[ARGS.prop] = ARGS.value;
    reply({ ok: true, prop: ARGS.prop, before: before, after: w[ARGS.prop] });
} catch (e) { reply({ ok: false, error: String(e) }); }
