// Tiling one window by its internal id: ARGS.id, ARGS.how = "quick-left" | "quick-right" |
// "custom-left" (KWin's custom tiling: split the output's root tile, put the window in the left
// half) | "maximize" | "untile".
try {
    const w = byId(ARGS.id);
    if (!w) throw new Error("no window " + ARGS.id);
    const before = describe(w);
    let note = "";
    if (ARGS.how === "quick-left" || ARGS.how === "quick-right") {
        workspace.activeWindow = w;
        if (ARGS.how === "quick-left") workspace.slotWindowQuickTileLeft();
        else workspace.slotWindowQuickTileRight();
    } else if (ARGS.how === "custom-left") {
        const tiling = workspace.tilingForScreen(w.output);
        const root = tiling.rootTile;
        if (root.tiles.length === 0) { root.split(1); note = "split the root tile (1 = horizontal)"; }
        const left = root.tiles[0];
        w.tile = left;
        note += "; root tiles now " + JSON.stringify(root.tiles.map(t => rect(t.absoluteGeometry)));
    } else if (ARGS.how === "maximize") {
        w.setMaximize(true, true);
    } else if (ARGS.how === "untile") {
        w.tile = null;
    }
    reply({ ok: true, how: ARGS.how, note: note, before: before, after: describe(w) });
} catch (e) { reply({ ok: false, error: String(e) }); }
