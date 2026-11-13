#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
X=${CRAFT_EXT_BIN:-$ROOT/craft-extensions/target/debug/craft-ext}
B=${EFFECTCRAFT_CLI:-$ROOT/effectcraft/target/debug/effectcraft-cli}
S=$(mktemp -d "${TMPDIR:-/tmp}/ec-plugin-smoke.XXXX")
echo "Smoke artifacts: $S"
export CRAFT_EXT_DIR=$S
for name in effectcraft-cue-board effectcraft-point-tool effectcraft-channel-tint; do
  "$X" dev "$ROOT/craft-extensions/examples/$name" --app effectcraft
done
export CRAFT_EXT_DIR=$S/development
"$B" plugin setup io.github.arnav-hiwarkar.effectcraft-cue-board --all --grant document.read:document --grant document.edit:document
"$B" plugin setup io.github.arnav-hiwarkar.effectcraft-point-tool --all --grant document.read:document --grant document.read:selection --grant document.edit:document
"$B" plugin setup io.github.arnav-hiwarkar.effectcraft-channel-tint --all
"$B" --empty run comp.new '{"name":"Main","width":32,"height":32,"duration":1}' layer.newSolid '{"name":"Solid","color":"#806040"}' --save-as "$S/base.ecproj"
"$B" --project "$S/base.ecproj" exec plugin.list > "$S/panels.json"
CUE_REV=$(python3 -c 'import json,sys; print(next(s["rev"] for s in json.load(open(sys.argv[1]))["surfaces"] if s["key"].endswith("effectcraft-cue-board/main")))' "$S/panels.json")
"$B" --project "$S/base.ecproj" run plugin.showPanel '{"surface":"io.github.arnav-hiwarkar.effectcraft-cue-board/main"}' plugin.uiEvent "{\"surface\":\"io.github.arnav-hiwarkar.effectcraft-cue-board/main\",\"rev\":$CUE_REV,\"widget\":\"add\"}" markers.list '{}' --save-as "$S/cue.ecproj"
"$B" --project "$S/base.ecproj" run plugin.showPanel '{"surface":"io.github.arnav-hiwarkar.effectcraft-cue-board/main"}' plugin.uiEvent "{\"surface\":\"io.github.arnav-hiwarkar.effectcraft-cue-board/main\",\"rev\":$CUE_REV,\"widget\":\"add\"}" edit.undo '{}' markers.list '{}' --save-as "$S/cue-undo.ecproj"
"$B" --project "$S/base.ecproj" run layer.select '{"layers":["Solid"]}' plugin.io.github.arnav-hiwarkar.effectcraft-point-tool.main '{}' plugin.toolEvent '{"phase":"begin","at":[4,8]}' plugin.toolEvent '{"phase":"update","at":[10,15]}' plugin.toolEvent '{"phase":"commit","at":[20,24]}' prop.get '{"layer":"Solid","path":"transform/position"}' --save-as "$S/tool.ecproj"
"$B" --project "$S/base.ecproj" run layer.select '{"layers":["Solid"]}' plugin.io.github.arnav-hiwarkar.effectcraft-point-tool.main '{}' plugin.toolEvent '{"phase":"begin","at":[4,8]}' plugin.toolEvent '{"phase":"commit","at":[20,24]}' edit.undo '{}' prop.get '{"layer":"Solid","path":"transform/position"}' --save-as "$S/undo.ecproj"
"$B" --project "$S/base.ecproj" run layer.select '{"layers":["Solid"]}' plugin.io.github.arnav-hiwarkar.effectcraft-point-tool.main '{}' plugin.toolEvent '{"phase":"begin","at":[4,8]}' plugin.toolEvent '{"phase":"cancel","at":[20,24]}' prop.get '{"layer":"Solid","path":"transform/position"}' --save-as "$S/cancel.ecproj"
"$B" --project "$S/base.ecproj" run layer.select '{"layers":["Solid"]}' effect.apply '{"effect":"io.github.arnav-hiwarkar.effectcraft-channel-tint/main"}' --save-as "$S/tint.ecproj"
"$B" --project "$S/base.ecproj" run layer.select '{"layers":["Solid"]}' effect.apply '{"effect":"io.github.arnav-hiwarkar.effectcraft-channel-tint/main"}' edit.undo '{}' --save-as "$S/tint-undo.ecproj"
"$B" --project "$S/tint.ecproj" render-frame --out "$S/tint.png"
"$B" --project "$S/tool.ecproj" render-frame --out "$S/tool.png"
"$B" --project "$S/base.ecproj" render-frame --out "$S/base.png"
python3 - "$S" <<'CHECK'
import json, pathlib, sys
root=pathlib.Path(sys.argv[1])
def comp(file):
    d=json.loads((root/file).read_text())
    return next(i['kind'] for i in d['items'].values() if i['kind']['type']=='Comp')
def group(c,key):
    return next(g for g in c['layers'][0]['props']['children'] if g['match']==key)
def position(c):
    return next(p for p in group(c,'transform')['children'] if p['match']=='position')
assert len(comp('cue.ecproj')['markers'])==1
assert not comp('cue-undo.ecproj')['markers']
keys=position(comp('tool.ecproj'))['keys']
assert len(keys)==1 and keys[0]['value']['v'][:2]==[20,24], keys
assert not position(comp('undo.ecproj')).get('keys',[])
assert not position(comp('cancel.ecproj')).get('keys',[])
assert len(group(comp('tint.ecproj'),'effects')['children'])==1
assert not group(comp('tint-undo.ecproj'),'effects')['children']
assert (root/'base.png').read_bytes() != (root/'tint.png').read_bytes()
for name in ['base.png','tint.png','tool.png']:
    assert (root/name).read_bytes().startswith(bytes([137,80,78,71,13,10,26,10]))
print('Document, undo and PNG assertions passed')
CHECK
echo "SMOKE PASS $S"
