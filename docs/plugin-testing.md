# Testing Craft plugins in EffectCraft (beta)

These steps repeat the beta flows against the local signed test catalogue that
`craft-extensions/scripts/e2e.sh` writes to `../test-catalogue/` (beside this checkout).
Every command below was run on Linux with debug builds. Paths assume this checkout is
`~/projects/plugin-beta/effectcraft`; adjust `ROOT` if yours differs.

## Setup

```sh
ROOT=~/projects/plugin-beta
# Build (the shared lock serialises cargo with other checkouts in the beta workspace).
cd $ROOT/effectcraft
flock $ROOT/.cargo.lock env CARGO_BUILD_JOBS=4 cargo build -p effectcraft-cli -p effectcraft
B=$ROOT/effectcraft/target/debug/effectcraft-cli

# A throwaway plugin store and the local catalogue.
export CRAFT_EXT_DIR="$(mktemp -d)"
export CRAFT_CATALOGUE_URL="file://$ROOT/test-catalogue/"
export CRAFT_CATALOGUE_ALLOW_FILE=1      # harmless for a file:// catalogue; needed for an https one with local assets
H=io.github.arnav-hiwarkar.halftone
N=io.github.arnav-hiwarkar.color-notes
F=io.github.arnav-hiwarkar.film-looks
```

The desktop app reads the same variables, so launching
`$ROOT/effectcraft/target/debug/effectcraft` from this shell uses the same store and catalogue.

If the catalogue was regenerated with a lower serial than one your store has seen,
`plugin refresh` refuses it as a possible replay: start again with a new `CRAFT_EXT_DIR`.

## 1. Manage plugins

```sh
$B plugin refresh                    # "Catalogue refreshed"
$B plugin search halftone            # halftone (latest 2.0.0, incompatible on purpose) and film-looks
$B plugin install $H                 # installs 1.0.0, the newest compatible release; never enables
$B plugin permissions $H             # none
$B plugin setup $H --all             # enables the effect
$B plugin install $N
$B plugin permissions $N             # required document.read:selection, optional document.edit:document
$B plugin setup $N --all --grant document.read:selection
$B plugin list
```

GUI: **Plugins ▸ Manage Plugins…** opens Settings ▸ Plugins (Browse / Installed / Updates /
Problems). Search, Download, then Enable opens the setup sheet listing each permission.

## 2. Effect

```sh
cd "$(mktemp -d)"
$B --empty run comp.new '{"name":"Main","width":320,"height":180,"duration":2}' \
  layer.newSolid '{"name":"Grey","color":"#808080"}' --save-as base.ecproj
$B --project base.ecproj render-frame --out base.png        # flat grey
$B --project base.ecproj run layer.select '{"layers":["Grey"]}' \
  effect.apply "{\"effect\":\"$H/halftone\"}" --save-as fx.ecproj
$B --project fx.ecproj render-frame --out fx.png             # black/white dots
$B --project fx.ecproj set - Grey effects/#1/size 20 --save  # change a form value and save
$B --project fx.ecproj props - Grey --flat | grep effects/   # size 20, angle 45, mono true after reopen
grep -A8 '"plugins"' fx.ecproj                               # the recorded id, version, sha256, contributions
```

Missing plugin: data is kept, previews pass through, final renders refuse.

```sh
$B plugin disable $H halftone        # or: $B plugin remove $H
$B --project fx.ecproj props - Grey --flat | grep effects/   # values still there
$B --project fx.ecproj exec plugin.missing                   # names the effect and plugin
$B --project fx.ecproj render-frame --out x.png              # refused: names the plugin, install and setup commands
$B --project fx.ecproj render --out x.png --format png --end 0.1   # refused the same way
$B plugin project fx.ecproj                                  # per-plugin status and the fix command
$B plugin setup $H --all             # (after remove: $B plugin install $H first)
$B --project fx.ecproj render-frame --out back.png           # identical to fx.png again
```

Kill list: build a second catalogue whose `revocations.toml` names halftone, with a higher
serial, then refresh from it. Renders refuse; `plugin project` says "release withdrawn".

```sh
X=$ROOT/craft-extensions/target/debug/craft-ext
mkdir kill-assets kill-cat
cp $ROOT/test-catalogue/assets/*.craftplugin kill-assets/
cp $ROOT/test-catalogue/index.json kill-cat/                 # so the new serial is higher
printf '[[revocation]]\nid = "%s"\nseverity = "disable"\nreason = "kill-list test"\ndate = "2026-11-13"\n' $H > kill-assets/revocations.toml
$X catalogue build --local-assets kill-assets --out kill-cat
$X catalogue sign --repo kill-cat --key ~/.config/craft/catalogue.key
CRAFT_CATALOGUE_URL="file://$PWD/kill-cat/" $B plugin refresh
$B --project fx.ecproj render-frame --out x.png              # refused
```

After this the store has seen the kill catalogue's serial; use a new `CRAFT_EXT_DIR` for the
other flows.

GUI: open `fx.ecproj`. **Effect ▸ Stylize ▸ Halftone** (or Effects & Presets, search
"Halftone") adds the effect; Effect Controls shows Dot size (px), Grid angle (°) and Black and
white. With the plugin off, opening the project shows "This project uses 1 plugin…" with
Continue / Open without; the effect stays in Effect Controls as a placeholder and
**File ▸ Export ▸ Add to Render Queue** then Render refuses with the same message as the CLI.

## 3. Session plugin (color-notes)

```sh
CMD=plugin.$N.add-marker
# Without document.edit:document: the command finishes, adds nothing, and the panel says why.
$B --project base.ecproj run layer.select '{"layers":["Grey"]}' $CMD '{}' plugin.list '{}' edit.history.list '{}'
$B plugin grant $N document.edit:document
# With it: one marker and one undo step per event.
$B --project base.ecproj run layer.select '{"layers":["Grey"]}' $CMD '{}' $CMD '{}' \
  markers.list '{}' edit.undo '{}' markers.list '{}' edit.history.list '{}'
# The panel button, as the GUI sends it (rev taken from plugin.list in the same batch):
$B --project base.ecproj run layer.select '{"layers":["Grey"]}' $CMD '{}' engine.batch \
  "{\"atomic\":false,\"steps\":[{\"command\":\"plugin.list\"},{\"command\":\"plugin.uiEvent\",\"params\":{\"surface\":\"$N/notes\",\"widget\":\"add-marker\",\"rev\":\"\$1.surfaces.0.rev\"}}]}" \
  markers.list '{}'
```

GUI: **Plugins ▸ Color notes ▸ Color notes** opens the floating panel (selected layer names
and an "Add note marker" button); **Plugins ▸ Color notes ▸ Add note marker** runs the command.
Each press is one **Edit ▸ Undo** step named "Add note marker · Color notes". Revoke the edit
permission in Settings ▸ Plugins and press the button again: the panel shows the
permission notice and nothing changes.

## 4. Data plugin (film-looks)

```sh
$B plugin install $F --version =1.0.0 && $B plugin setup $F --all
$B --empty exec plugin.resources        # the film-warm LUT path and two halftone presets
$B --project base.ecproj run layer.select '{"layers":["Grey"]}' \
  plugin.$F.print-looks.halftone-print '{}' edit.history.list '{}' --save-as preset.ecproj
#   needs halftone installed and enabled; otherwise it fails naming the missing effect
LUT=$($B --empty exec plugin.resources --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["luts"][0]["path"])')
$B --project base.ecproj run layer.select '{"layers":["Grey"]}' effect.apply '{"effect":"ec.utility.applylut"}' --save-as lut.ecproj
$B --project lut.ecproj set - Grey effects/#1/lut "\"$LUT\"" --save
$B --project lut.ecproj render-frame --out lut.png       # warm grey
$B plugin updates; $B plugin update $F                    # 1.1.0, enablement kept
$B plugin rollback $F 1.0.0
```

GUI: **Plugins ▸ Film looks ▸ Halftone print** applies the preset to the selected layers as one
undo step. For the LUT, add **Effect ▸ Utility ▸ Apply Color LUT** and choose the path printed
by `plugin.resources`.

## 5. Contract

```sh
$B plugin contract | diff - $ROOT/craft-extensions/docs/plugins/contracts/effectcraft.md
```

## Known limits

- Plugin wasm must be release-built. Debug modules (even at opt-level 1) exhaust the
  per-pixel fuel budget, and every render then fails with "the plugin took too long".
- Halftone renders single-threaded in the interpreter: about 17 s for a 1920×1080 frame in this
  debug app build. Use small comps or Draft/Half resolution for interactive testing.
- A saved Apply Color LUT path points into the release directory of the version that was
  active; after update/rollback and pruning, re-pick the LUT.
- Safe mode (`--safe-mode` or `EFFECTCRAFT_SAFE_MODE=1`) loads no plugin code.
- GUI flows were not exercised by automated tests in this round (no display); the CLI drives the
  same engine commands.
