// The window list as KWin scripting sees it, plus the API surface (property and method names).
try {
    const wins = workspace.windowList().filter(w => w.normalWindow).map(describe);
    const any = workspace.windowList().find(w => w.normalWindow);
    const tiling = any ? workspace.tilingForScreen(any.output) : null;
    reply({ ok: true, windows: wins, outputs: workspace.screens.map(o => o.name),
            windowKeys: any ? Object.keys(any).sort() : [],
            tilingKeys: tiling ? Object.keys(tiling).sort() : [],
            rootTileKeys: tiling ? Object.keys(tiling.rootTile).sort() : [],
            rootTiles: tiling ? tiling.rootTile.tiles.map(t => rect(t.absoluteGeometry)) : [] });
} catch (e) { reply({ ok: false, error: String(e) }); }
