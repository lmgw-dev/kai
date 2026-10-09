// Prepended to every WP13 KWin script by kwin_script.py. ARGS is filled in by the caller; the
// result goes back over D-Bus to the caller's own name (the path Kai would use: KWin scripts
// cannot return a value to loadScript's caller, and print() lands in KWin's log).
const ARGS = __ARGS__;
function reply(obj) {
    callDBus(ARGS.reply_to, "/", "dev.rootshell.KaiP3", "Result", JSON.stringify(obj));
}
function rect(g) { return g ? { x: g.x, y: g.y, width: g.width, height: g.height } : null; }
function describe(w) {
    return {
        id: w.internalId.toString(), caption: w.caption, resourceClass: w.resourceClass,
        desktopFileName: w.desktopFileName, pid: w.pid, normal: w.normalWindow,
        frame: rect(w.frameGeometry), output: w.output ? w.output.name : null,
        active: workspace.activeWindow === w, maximizable: w.maximizable,
        tile: w.tile ? rect(w.tile.absoluteGeometry) : null, quickTileMode: w.quickTileMode,
    };
}
function byId(id) {
    for (const w of workspace.windowList()) if (w.internalId.toString() === id) return w;
    return null;
}
