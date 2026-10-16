# Chat references and direct Home commands

Type `/` in the existing scene-assistant input to choose an operation. Use
Arrow keys then Tab/Enter, or click a suggestion. `@` selects a discovered peer
by alias; `#` selects a note, volume, or positioned portal in the selected local
scene. Duplicate names show their type and ID. Names with spaces are quoted.

The same autocomplete works anywhere in an ordinary Qwen message: `@` for
peers and `#` for scene nodes. For example, type `How far is @Dan from #cheese?`
and choose each reference. The conversation shows the readable message; Qwen
also receives the exact selected IDs and their scene/frame as reference data.
Those selections remain in conversation history, but are not cached poses or
proof of current existence: Qwen's tools still resolve and validate current state.
Editing a reference or switching scenes invalidates its selection. Unselected
references must be selected before sending; normal messages need no selection.
Enter/Tab accepts a suggestion, arrows navigate, and Escape dismisses it.
Ordinary messages go to Qwen; only a leading `/` executes directly.

Examples (choose the references from suggestions, not just pasted names):

```text
/camera @"Dan's Phone" "camera-product-id"
/attach @"Dan's Phone" "camera-product-id"
/detach
/focus #"cheese"
/focus @"Dan's Phone"
/path @"Dan" @"Bracket 0125"
/path @"Dan" #"cheese"
/path #"Kitchen" #"cheese"
/path #"1" #"2" #"3" @"Dan" @"Bracket 0125"
/inspect @"Dan's Phone"
/rename @"Dan's Phone" "Dan"
/rename #"cheese" "Cheddar"
/move #"cheese" 2 0.1 -1
/resize #"Kitchen" 4 2.5 3
/note "this is Dan" 2 0.1 -1
```

Camera suggestions come from the selected peer's existing SDK Catalog; select
one exact Product. Attach requires a registered camera frame. Focus on a peer
uses Home's generic live pose resolver, not a cached label. Lost tracking or a
missing transform is an error. Camera/view commands change this browser's view,
not the physical device. Resize preserves the volume's bottom, as in the
existing tool. Coordinates and dimensions are XYZ metres in the selected scene.

`/path` accepts a variable-length ordered list of independently selected `@peer`
or `#entity` waypoints (2–64 per request). Continue typing `#` or `@` to add
another waypoint; Send draws one continuous path through them in order.
It resolves each distinct peer once through the same registry-backed reader as
the viewer, and projects all positions onto the selected scene's floor (Y=0, line lifted
6 cm). It uses Annotator's visual-route policy: 0.25–1.9 m obstacle-height band,
25 cm soft clearance preference, eight-neighbour A*, preference for observed
floor, and a bounded 320×320-cell search per leg with 3 m padding. The total
search and output are also bounded. Unobserved floor is
allowed, as in Annotator, but explicitly reported in the receipt. This is visual
guidance, **not a verified robot navigation route or a movement command**.

Unlike Annotator's current fallback, blocked endpoints and failed/oversized
searches produce an error, never a straight line through obstacles or a shifted
destination. Stale/lost/untransformed peer poses also prevent saving. Every leg
must succeed before anything is saved; failures identify the offending leg.
Intermediate waypoints are preserved, not reordered or optimized away.
A path is a snapshot, not a live tether; run the command again to recompute it.

Paths use the existing `spatial-harness.scene-paths/v1` storage and SDK Product,
with Annotator's pink 5 cm line. Home and Annotator consume the same saved path;
the normal path selection/delete controls apply. No new peer transport, schema,
phone build or robot operation is involved. `create_path` lives in the shared
scene-tool dispatcher; this change exposes it through `/path`, not model routing.

`/point` is **not exposed as a direct command**. The existing Qwen robot-pointing
workflow is unchanged; these slash commands do not invoke it. Direct commands
also deliberately omit deletion, bulk clearing and model-dependent image descriptions.

## Execution

- `GET /api/commands?scene=...` projects current scene entities and Catalog camera
  declarations into autocomplete options; it is not another discovery registry.
- The input binds each selection to an ID (and a full Product reference for a
  camera). Editing a reference invalidates the binding; switching scenes clears
  all bindings. Ordinary text and finite numeric arguments are parsed locally.
- `POST /api/commands` accepts only a named command and typed arguments, plus
  the selected scene/frame and viewer context. Home validates these against
  freshly read state, reads edit targets again, and passes expected current
  values to the **same `scene_tool` dispatcher used by Qwen**.
- No command path calls Ollama or submits a synthetic natural-language request.
  Unknown commands never fall back to Qwen. Direct interactions are labeled
  “Home” and are not added to Qwen's conversation history.
- The existing Host/same-origin write protection, scene ownership, tool
  validation and SDK/libp2p paths remain in force. At most one edit is made per
  command. An uncertain operation is not automatically retried.
- Browser-to-Home HTTP is unchanged. No new peer transport is introduced.

## Validation

```sh
cargo test --locked --offline --test slash_commands --test scene_chat_tools --test chatbot_volumes
cargo test --locked --offline --bin spatial-home command_tests
node --test tests/slash-commands.test.mjs
npx playwright test -c playwright.commands.config.js
```

Browser tests intercept every request with fixtures, including the full Home UI
and PiP/view actions. They do not start another Home or contact live devices.
Server-handler tests run in process against temporary data, without listeners.
