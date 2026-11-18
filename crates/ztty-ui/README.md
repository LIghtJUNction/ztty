# ZUI/1 native terminal UI

An experimental, bounded, declarative scene protocol for ztty. It has no browser,
JavaScript runtime, network access, shell execution, or image decoder. The host
renders trusted native components and owns input routing and session permissions.

The wire format is `ESC _ ZUI;1;<UTF-8 JSON> ESC \\`. Each complete frame is one
transaction. Invalid, cancelled, unsupported, or oversized frames preserve the
previous scene. Ordinary terminal data and Kitty `_G` graphics keep their own paths.

## Messages

`capabilities` has `v:1` and `op:"capabilities"`. Its response reports `last_rev`,
the component catalog and limits. Read this before starting a new application.
Generic zterm consumers default to `supported:false` and ignore scene mutations.
An attached native frontend must call `Terminal::set_ui_host_enabled(true)` before
pumping the PTY. Only that frontend advertises `supported:true`. Applications use
text fallback on an unsupported response or probe timeout. Detaching the renderer
clears the scene and cancels an incomplete frame.

`replace` requires `v`, `op`, nonzero numeric `surface`, `rev`, `root`, and `nodes`.
`events` defaults to false. Nodes have stable nonzero numeric `id` and a `type`.
The complete tree must have one root, no missing children, sharing, cycles or
unreachable nodes. Nodes are stored by ID; `children` supplies display order.

`update` requires `surface`, `base_rev`, a newer `rev`, and `set`, a nonempty list
of existing node IDs with changed properties. Types and IDs cannot change. All
updates are validated together before any are committed. Use `replace` to add or
remove nodes, change types or renegotiate events.

`clear` requires `surface`, `base_rev` and a newer `rev`. Revisions are unsigned
64-bit integers and strictly increase across the terminal lifetime, including
clear, RIS and primary/alternate screen switches. Replayed revisions are rejected.
Screen switches and RIS dismiss the scene without resetting the revision floor.

## Trusted catalog

| Type | Properties |
| --- | --- |
| `column`, `row` | `children`: node ID array |
| `text` | `text`, optional `style`: `body`, `heading`, `muted`, `code` |
| `progress` | integer `value` from 0 to 10000, optional `label` |
| `button` | `label`, `action`, optional `enabled` (default true) |
| `image` | nonzero `image_id`, optional `generation`, `alt`, `cols`, `rows` |

Images reference the current terminal's existing KGP image store. No pixels are
stored here. Omitted generation follows that image ID's current content; a supplied
generation pins exactly those contents. Missing images use `alt`/placeholder.
The host controls fonts, theme, clipping and panel geometry. v1 does not reserve
terminal rows or insert components into scrollback.

## Bounds and actions

Maximum JSON frame: 65536 bytes; nodes: 256; tree depth: 8 (root is depth 1);
aggregate scene text: 32768 UTF-8 bytes; individual string: 4096 bytes; action:
64 ASCII letters, digits or `._:-`; image extents: 1..1024 cells. Progress uses
integer basis points. Unknown fields and duplicate keys are errors. Text rejects
control characters except newline and tab. The receiver does not execute escapes.

Only `events:true` scenes can yield actions. The host passes the surface/revision
captured from the displayed hit to `State::action` and additionally verifies live
session ownership, current layout, and a complete press/release gesture. Actions
are intents returned to the host, not automatically written to a PTY. An opted-in
application can receive the same APC framing with JSON fields `v:1`,
`type:"action"`, `surface`, `rev`, `node`, `action`, `event_id`. Event IDs never wrap.

This is a native ztty protocol, not a claim of MCP Apps HTML/iframe compatibility
or complete A2UI/OpenUI conformance. Protocol tests and the named benchmark
scenarios accompany the implementation; no performance ranking is implied.

## Complete exchange

After a supported capability response with last_rev:0, wrap each JSON object
below in the APC prefix and ST terminator described above. For an existing
terminal, substitute the reported revision plus one.

~~~json
{"v":1,"op":"replace","surface":7,"rev":1,"root":1,"events":true,"nodes":[{"id":1,"type":"column","children":[2,3,4]},{"id":2,"type":"progress","value":2500,"label":"Building"},{"id":3,"type":"image","image_id":42,"alt":"Preview"},{"id":4,"type":"button","label":"Retry","action":"build.retry"}]}
{"v":1,"op":"update","surface":7,"base_rev":1,"rev":2,"set":[{"id":2,"value":10000,"label":"Complete"}]}
~~~

The image bytes must already have been uploaded through KGP; image_id:42 is
only a reference. A click on the displayed button can yield this opted-in reply:

~~~json
{"v":1,"type":"action","surface":7,"rev":2,"node":4,"action":"build.retry","event_id":1}
~~~

A mismatched base_rev, stale click, invalid graph or invalid property does not
partially apply. The old scene remains usable. Mutation and event counters fail
closed at unsigned 64-bit exhaustion instead of wrapping or reusing identities.

## Measuring the core

Run cargo bench -p ztty-ui --bench protocol. The benchmark emits JSON lines for
256-node creation, single-node progress updates in a 256-node tree, capability
parsing, oversized-frame rejection, image-reference creation, and byte-fragmented
frame collection. Fixtures are constructed before measurement. Allocation hooks
exist only in the benchmark executable; production code forbids unsafe code.

The output reports allocations, reallocations and requested allocation bytes,
not peak live memory or RSS. Timings include the allocator instrumentation.

A batch whose `set` entries contain only `id` and integer `value` fields in
`0..=10000` can update progress values in place. It verifies the surface and
revision, each target's ID and type, and duplicate IDs before any mutation.
Existing labels, tree structure and retained text stay unchanged, so this path
reuses their earlier validation. Updates with other node properties retain the
general staging and validation path. The parser still allocates its entries
vector, even for a single numeric update.

Record the commit, toolchain, CPU and repeated samples before comparing results.
The existing zterm benchmark separately checks ordinary VT throughput.
