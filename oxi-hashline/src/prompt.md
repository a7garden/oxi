# Hashline Patch Format — Model Reference

This is the ONLY patch format you may use with the `edit` tool.
Every section is anchored on a **[PATH#TAG]** header from your most recent
`read` or `search`. The tag certifies the exact file version you are editing.

───
## Headers

`[PATH#TAG]` — every section MUST start with the `[path#TAG]` header
from your latest read/search. The TAG is a 4-hex content hash.

───
## Ops

`SWAP N.=M:` — replace original lines N..M (inclusive) with the body rows.
`DEL N.=M` — delete original lines N..M (inclusive). No body.
`DEL N` — delete a single line N. No body.
`INS.PRE N:` — insert body rows immediately before line N.
`INS.POST N:` — insert body rows immediately after line N.
`INS.HEAD:` — insert body rows at the very start of the file.
`INS.TAIL:` — insert body rows at the very end of the file.

───
## Body Rows

Every row under a `:` header is `+TEXT` — a literal line of final content.
`+` alone adds a blank line.
There are NO `-old` rows; the op line determines what is deleted.

───
## Rules

- Line numbers come from your LATEST read/search output. The `[PATH#TAG]`
  header names the file version. Stale tag or surprise? STOP, re-read.
- RANGES ARE TIGHT. Cover ONLY lines that change. Never widen over
  untouched lines — a stale wide range shreds everything it spans.
- THE BODY IS THE FINAL CONTENT. Only `+TEXT` rows; never `-old` or
  context lines. The op's range does the deleting.
- Every applied edit mints a FRESH `[PATH#TAG]` and RENUMBERS. Anchor the
  next edit on the edit response or a fresh read — never on memory.
- Insert at, never inside: use `INS.PRE N` / `INS.POST N` with a target
  line number from your latest read. Do not edit around your inserts.
- Multi-section patches are ALL-OR-NOTHING. All sections preflight then
  commit together.

───
## Example

```
[src/main.rs#A1B2]
SWAP 5.=7:
+    let config = load_config()?;
+    let app = App::new(config);
+    app.run()
INS.POST 10:
+    // Validate input before processing.
INS.HEAD:
+//! Application entry point.
DEL 15
```

───
## Anti-Patterns

- WRONG: `SWAP 1.=2:` with body `+def greet(name):` (off-by-one).
  RIGHT: `SWAP 1.=1:` — body length is independent of range width.
- WRONG: `-old: line` or bare context lines. Body is ONLY `+TEXT`.
- WRONG: Using a stale tag from an old read. Re-read before every edit.
- WRONG: Widening a range over unchanged lines. Touch only what changes.
- WRONG: `INS.BLK.POST N:` on a closing delimiter. Use plain `INS.POST M:`.
- WRONG: Inserting as a widened `SWAP` that retypes keepers.
  RIGHT: Use `INS.PRE` / `INS.POST` — touch nothing you keep.
