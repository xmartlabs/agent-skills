# Visual feedback

Use this workflow to apply visual feedback to a flow or a specimen. Notes come from Agentation,
which the designer uses to click an element on the running design space and leave a note.

1. Read pending notes through the Agentation MCP server (`agentation_get_all_pending`). If the
   server or its local endpoint is unavailable, say so and ask the designer to paste Agentation's
   copied output; never make up notes.
2. Use each note's page, element, component and property context to find the source location in
   `flows/`, `specimens/` or `library/`. A note's source file may point at a shell file such as
   `src/shell/Specimen.tsx`, which is engineer-owned and not what to edit. Map it through the
   page's route instead: `#/components/<name>` is `specimens/<name>.tsx`, `#/shared/<name>` is
   `library/<name>.tsx` and `#/preview/<flow>/...` is `flows/<flow>/`. A note on the shell
   itself (the sidebar, a page title) is an engineer request: say so and stop.
3. For a component property, change it where the flow or specimen uses the component. Never restyle
   the product component itself; that is a proposal (see [flows.md](flows.md)).
4. For a style change, use an existing token. Map a captured raw value to a token only when the
   match is clear; otherwise ask before adding one (see [tokens.md](tokens.md)).
5. Keep the note pending until the change passes `npm run design-space:check` and looks right at
   every device width the toolbar offers (on a prototype preview) or a few viewport widths (other
   pages).
6. Resolve a note only after that, with `agentation_resolve`.

If a note asks for a brand decision, a change to product behavior or a component that does not
exist, stop and route it to the right workflow before resolving it.
