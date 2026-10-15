# Compact compiled TUI storyboard

Open [storyboard.html](storyboard.html) for 36 actual compiled fixture screenshots: sessions and quota at 120×36,76×30,48×24, each showing browse, provider chooser, contextual Help, applied Claude filter, cancelled draft and Details. The [manifest](help-image-manifest.json) binds all PNGs and six capture receipts.

The gallery captures version 0.1.96 at base commit d448337c plus uncommitted review corrections, source snapshot `903a19b5b39192487a35c9dd968a1aeb14322f066a7d749649a9bae4e3d7f468`. It was captured on 2026-10-15 after restoring the original narrow account columns and correcting Help. The inputs are synthetic; input and rendering use the compiled TUIs. Provider requests are zero. This is local UI proof, not published-head CI or configured-native execution.

Machine/provider share one row; the session title carries the count; empty search and default view/sort are omitted. Help belongs in the title row. Quota chrome uses title plus two supporting rows; filters retain provider, disabled visibility and exact visible/total count.

[Earlier 93-image 0.1.95 gallery](storyboard-0.1.95-archived.html) and [earlier d448 baseline](storyboard-0.1.96-d448-baseline.html) are historical evidence. See [validation.md](validation.md) for current results, prior snapshot evidence and remaining gates.
