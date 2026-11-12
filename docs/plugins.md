# Plugins and extensions

EffectCraft's core aims at After Effects parity. What After Effects users get from third parties
(effect plug-ins, scripts, ScriptUI panels) is an extension in EffectCraft too, and EffectCraft
offers the same kinds of extension points as After Effects so that anyone can write their own.

## Extension points compared with After Effects

From After Effects' public documentation (the user guide's scripting pages and the Scripting
Guide):

| After Effects | EffectCraft | |
|---|---|---|
| Effect plug-ins (C/C++ SDK, `.aex` / `.plugin`) | [Craft plugins](#craft-plugins): sandboxed WebAssembly effects, commands, panels, event hooks, presets and LUTs, shared with the other Craft apps | Different ABI: After Effects SDK plug-ins can't run (G8) |
| Scripts folder: its scripts are listed in File ▸ Scripts | `Scripts/` in the settings folder (File ▸ Scripts ▸ Install Script File… copies a script there), listed in File ▸ Scripts with the bundled samples; a subfolder (one level) is a submenu named after it | Same |
| File ▸ Scripts ▸ Run Script File… (`.jsx`, `.jsxbin`) | `.jsx` / `.js` | Gap: `.jsxbin` (an undocumented binary encoding) |
| File ▸ Scripts ▸ Recent Script Files: the 10 most recently run scripts; Cmd+Option+Shift+D (Ctrl+Alt+Shift+D on Windows) reruns one | File ▸ Scripts ▸ Recent Script Files lists the 10 scripts run last (script files and File ▸ Scripts entries, newest first, kept in the settings); Cmd+Alt+Shift+D runs the most recent one again (`file.runScript {"recent": 0}`, a key that can be changed in Edit ▸ Keyboard Shortcuts) | Same |
| ExtendScript preprocessor directives (`#include`, `#target`, `#targetengine`) | `#include` / `//@include` / `#includepath` inline files relative to the script (once each, depth-limited, file gate applies); `#target`, `#targetengine`, `#strict` are ignored | Same |
| `Scripts/Startup` and `Scripts/Shutdown`: run in alphabetical order when the app starts and quits | `Scripts/Startup/` and `Scripts/Shutdown/` in the settings folder, in alphabetical order (case ignored): the Startup scripts run at launch before a project opens, and the Shutdown scripts run as the desktop app quits, after the project is closed. A Startup script that fails is reported in a toast and in the log, a Shutdown script that fails is reported in the log, and the next script still runs | Partial: each script runs in its own global scope, so variables and functions a Startup script defines are not available to later scripts; a Startup script's modal dialog stays open while the next scripts run and the app starts; a Shutdown script's modal dialogs close as if canceled; the web app runs no Shutdown scripts |
| `Scripts/ScriptUI Panels`: listed at the bottom of the Window menu, dockable like the app's own panels, `this` is the panel | `Scripts/ScriptUI Panels/` in the settings folder (File ▸ Scripts ▸ Install ScriptUI Panel… copies one there), listed at the bottom of the Window menu (a subfolder, one level, is a submenu named after it), docked like any panel, `this` is the panel; the panels open when the app quits open again at the next launch | Same. Where a panel was docked isn't kept: workspace layouts aren't saved between launches |
| Keyboard shortcuts for scripts (the Keyboard Shortcuts editor lists the scripts of the Scripts folder) | Edit ▸ Keyboard Shortcuts lists the scripts of File ▸ Scripts under File ▸ Scripts (`file.runScript {"name": "<file name>"}`); a key given to one is saved with the preset, File ▸ Scripts shows it, and pressing it runs the script as its menu entry does | Same. After Effects binds a key to a script's place in the alphabetical list (`ExecuteScriptMenuItem01`…), and EffectCraft binds it to the script's file name. The Window menu entries of ScriptUI panels aren't offered |
| Running a script from the command line (`afterfx -r script.jsx`) | `effectcraft-cli script`, the control channel's and MCP's `script.run` | Same purpose, other entry points |
| `app.settings` (kept in the preferences between sessions) | Kept in `script_settings.json` in the settings folder (`script.settings.get` / `script.settings.save`) | Same |
| Allow Scripts to Write Files and Access Network | Settings ▸ Scripting & Expressions | Same |

The settings folder is `~/Library/Application Support/EffectCraft` on macOS, `%APPDATA%\EffectCraft`
on Windows and `$XDG_CONFIG_HOME/effectcraft` (or `~/.config/effectcraft`) on Linux
([preferences.md](preferences.md)); the web app keeps it in browser storage.

## Scripts

Scripts use After Effects' scripting object model (`app`, `app.project`, `CompItem`, layers,
`Property` with keyframes and eases, `app.beginUndoGroup`…), so most scripts written for After
Effects' documented API run unchanged. Every edit a script makes is an engine command: undoable,
journaled and the same as the UI's action. The `effectcraft-script` crate's docs
(`crates/script/src/lib.rs`) list the object model.

* Run one: File ▸ Scripts ▸ Run Script File…, the Script Console panel, `effectcraft-cli script
  file.jsx`, or `script.run {code, name}` over the control channel and MCP.
* Install one: File ▸ Scripts ▸ Install Script File… (or copy it into `Scripts/` in the settings
  folder). It is listed in File ▸ Scripts at once (After Effects asks for a restart).
* Run one at every launch or quit: copy it into `Scripts/Startup/` or `Scripts/Shutdown/` in the
  settings folder. These scripts are not listed in File ▸ Scripts. `file.runStartupScripts` and
  `file.runShutdownScripts` run them and return `{ran, waiting, failed}`; the desktop app calls them at
  launch and quit, and the web app calls `file.runStartupScripts` at launch.
* `file.scripts.list` lists installed and bundled scripts and panels; `file.runScript {name}` runs
  one; `file.uninstallScript {name}` removes an installed one.
* A subfolder of `Scripts/` is a submenu of File ▸ Scripts named after the folder, and its scripts
  are named `Folder/File.jsx` in `file.scripts.list` and `file.runScript`. One level of folders is
  listed; deeper folders are not. `file.uninstallScript` removes only scripts at the top of the
  Scripts folders: delete a script in a subfolder yourself. A subfolder may be a link to another
  folder (an extension's own), and uninstall never deletes a file through it.

## ScriptUI panels

A ScriptUI panel is a script that builds its user interface into the panel it is given as `this`.
Put it in `Scripts/ScriptUI Panels/` in the settings folder (or use File ▸ Scripts ▸ Install
ScriptUI Panel…) and it is listed at the bottom of the Window menu under its file name; choosing it
runs the script and docks its panel as a tab, which you can move, float, maximize and close like any
other panel. Closing the tab closes the panel (its `onClose` runs). Panels open when EffectCraft
quits open again at the next launch (`window.restoreScriptPanels`, which the desktop and web apps
run at start-up; the list is `scriptui_panels.json` in the settings folder).

To group panels, for example the several panels of one extension, make a subfolder of
`Scripts/ScriptUI Panels/` and put them in it: `Scripts/ScriptUI Panels/My Tools/Animate.jsx` is
listed as Window ▸ My Tools ▸ Animate.jsx, its tab is titled Animate, and commands name it
`My Tools/Animate.jsx` (`window.scriptPanel {name}`). One level of folders is listed. Install
ScriptUI Panel… copies a panel to the top of the folder; make the subfolder and move panels into it
yourself, and delete them from it yourself too.

Run from File ▸ Scripts instead, the same script gets no panel, so it should make a floating
palette itself. A minimal panel:

```js
// Nudge.jsx: moves the selected layers 10 px right, as a dockable panel.
(function (thisObj) {
  var ui = thisObj instanceof Panel ? thisObj : new Window("palette", "Nudge", undefined, { resizeable: true });
  ui.orientation = "column";
  ui.alignChildren = ["fill", "top"];
  var go = ui.add("button", undefined, "Nudge Right", { name: "nudge" });
  var status = ui.add("statictext", undefined, "Select layers, then click.");
  go.onClick = function () {
    var comp = app.project.activeItem;
    if (!(comp instanceof CompItem)) return;
    var layers = comp.selectedLayers;
    app.beginUndoGroup("Nudge Right");
    for (var i = 0; i < layers.length; i++) {
      var p = layers[i].transform.position;
      var v = p.value;
      if (p.numKeys === 0) p.setValue([v[0] + 10, v[1]]);
    }
    app.endUndoGroup();
    status.text = "Moved " + layers.length + " layer(s).";
  };
  if (ui instanceof Window) ui.show(); else ui.layout.layout(true);
})(this);
```

Supported controls: `panel`, `group`, `button`, `iconbutton`, `statictext`, `edittext`,
`checkbox`, `radiobutton`, `slider`, `scrollbar`, `progressbar`, `dropdownlist`, `listbox`,
`tabbedpanel` / `tab` and `image`, with ScriptUI's automatic layout (`orientation`,
`alignChildren`, `alignment`, `margins`, `spacing`, `preferredSize`), resource strings, event
handlers (`onClick`, `onChange`, `onChanging`, `onClose`, `addEventListener`) and `onDraw` with
ScriptUIGraphics. Mouse events (`addEventListener("mousedown" | "mouseup" | "mousemove" |
"mouseover" | "mouseout" | "click", handler, capture?)`) reach the control under the pointer and
its containers (capture, target, bubble; `stopPropagation()`), with a MouseEvent carrying
`clientX`/`clientY` relative to the target, `button`, `detail` (click count) and the modifier
keys. Agents use a panel like a user: `scriptui.list`, `scriptui.get` (its controls),
`scriptui.click`, `scriptui.set`, `scriptui.mouse` and `scriptui.close`; every control has the automation id
`scriptui.<window>.<control id or name>`. The bundled sample `Layer Tools.jsx`
(`crates/engine/scripts/ScriptUI Panels/`) and the Ease Presets extension below are longer
examples.

## Bundled extensions

Optional tools that ship with EffectCraft but are not part of its core: they are scripts on the
public scripting API, like the third-party scripts After Effects users install, and are listed
with the bundled samples (`file.scripts.list` reports them as `extension`). A script of the same
name in your Scripts folders takes their place.

### Ease Presets (`extensions/scriptui-panels/Ease Presets.jsx`)

Window ▸ Ease Presets.jsx: easing curves kept by name and applied to the sides of selected
keyframes (#254; it was a core panel in v0.6.0). A curve is the out side of a segment's
first key and the in side of its second, in the Keyframe Velocity dialog's terms: influence in
percent, and speed relative to the segment's average speed (1 = as fast as a straight line, 0 = at
rest), so one curve fits any duration and change of value.

* Click a preset to apply it to the selected keyframes of each selected property, in one undo
  step. **Ease** chooses the sides (#1329): **In** eases each key's incoming side with the curve's
  in side, **Out** its outgoing side with the curve's out side, **In-Out** (the default) both. A
  side is eased when it has a neighbouring keyframe, selected or not (the neighbour doesn't
  change), with its speeds scaled to the segment on that side: per dimension, and along the
  motion path for spatial properties. So a single keyframe works, a selected pair gets the whole
  curve, and the first and last keys ease the side that has a segment. The side not eased keeps
  playing as it did (a Hold stays a Hold). Roving keyframes are skipped, and the status says how
  many: they pass speed straight through, so an ease on one wouldn't play (turn off Roving to
  ease it). The keys stay selected, so you can try another.
* The drawing shows the working curve; type its numbers (Out % and Speed, In % and Speed) and
  click Apply, or click From Keys to read the curve between the first selected pair of neighbouring keys.
* Save keeps the working curve under the typed name (replacing a preset of that name); Rename
  and Delete act on the selected preset of your own. The twelve built-in presets can't change.
* User presets are kept with `app.settings` (section `Ease Presets`, key `userPresets`, in
  `script_settings.json` in the settings folder). Presets saved by the v0.6.0 core panel
  (`ease_presets.json`) move there the first time a later version loads its settings; the old
  file is left as it was.

It uses only After Effects' documented scripting API (`comp.selectedProperties`,
`Property.selectedKeys`, `keyInTemporalEase` / `keyOutTemporalEase`, `setTemporalEaseAtKey` with
`KeyframeEase`, `setInterpolationTypeAtKey`, `app.settings`, ScriptUI), so it is also a worked
example of a keyframe tool.

## Craft plugins

EffectCraft uses the shared Craft plugin platform
([craft-extensions](https://github.com/storytold/craft-extensions)); the earlier effect plug-in API,
the legacy plugin manager and external effect helpers were removed when it arrived.

- **Make a plugin:** the author guide, `docs/plugins/author-guide.md` in craft-extensions, and the
  examples there (`examples/halftone`, `examples/color-notes`, `examples/film-looks`).
- **What EffectCraft offers plugins** (contracts, operations and their permissions, events, where
  contributions appear): `docs/plugins/contracts/effectcraft.md` in craft-extensions, or
  `effectcraft-cli plugin contract`.
- **Use plugins:** Settings ▸ Plugins (also Plugins ▸ Manage Plugins…), or
  `effectcraft-cli plugin --help` (`search`, `install`, `setup`, `enable`, `grant`, `update`,
  `rollback`, `remove`…). Installing never turns anything on; permissions are granted per app.
- **Safe mode:** `effectcraft --safe-mode` (or `EFFECTCRAFT_SAFE_MODE=1`) runs no plugin code; the
  manager still works.
- **Agents:** `plugin.list`, `plugin.resources` (presets and LUT files), `plugin.missing`, and each
  plugin command as `plugin.<plugin id>.<contribution>`. Confined MCP sessions may only list.
- **Projects** record the plugins their effects use; opening one shows which are missing, and a
  missing plugin's effects keep their settings and pass the image through. The Render Queue refuses
  to render while an effect's plugin is missing.
