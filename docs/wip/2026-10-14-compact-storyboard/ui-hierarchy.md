# Terminal hierarchy

Sessions and interactive quota use the same visual roles from `codex-router-tui`. Heading level describes meaning; every terminal character keeps the same font size.

| Role | Appearance | Used for |
| --- | --- | --- |
| H1 — surface | Cyan, bold | The active page or dialog title |
| Inactive surface | Grey, bold | The page behind a modal |
| H2 — section | White, bold | Conversation, account provider, selected account and Help sections |
| H3 — group | Grey, bold | Smaller groups inside a section, such as Destination |
| Body | White | Content and available choices |
| Secondary | Grey | Metadata and explanatory text |
| Disabled | Dark grey | Unavailable actions, with an explanation |
| Outcome/status | Existing semantic color and explicit text | Success, failure, eligibility and refresh status |
| Selected | Yellow, bold, with a marker | The selected session, account, radio choice or draft |
| Focused action | Yellow, bold, underlined | Apply, Cancel or another action with keyboard focus |

Selection and keyboard focus have different meanings. Moving focus from a provider to Apply must not remove the provider's selected appearance. Color accompanies a marker or text; it does not carry the meaning alone.

Help is a plain action on the title row. Shortcut instructions appear inside Help, including the current dialog's instructions first. The normal page and dialog actions use plain labels. Details shares the main title row with Help; neither needs a footer. Enter opens the focused session/account Details. In sessions, Ctrl+Enter resumes and Ctrl+Shift+Enter opens fork confirmation; the old Alt+Enter action is retired. Details actions operate on the captured identity, including Copy session ID. Plain Enter in an existing chooser/confirmation still selects or applies.

Dialogs use the shared iocraft frame and input scope. At narrow sizes they fill the available viewport within the configured margins; at wider sizes they are framed overlays. Their title and actions remain fixed while the body scrolls. Opening Help retains the underlying draft and focused identity, blocks background input, and closing it restores the prior screen.

Static quota output must preserve its existing noninteractive presentation. The [current gallery](storyboard.html) contains 36 compiled fixture pictures captured on 2026-10-16 after the no-footer and Details-action changes, with the exact source snapshot in its manifest. Narrow account columns use the original allocation; Details retains full identity. The [93-image 0.1.95 archive](storyboard-0.1.95-archived.html) remains historical. See [validation.md](validation.md) for exact source, results and unverified gates.
