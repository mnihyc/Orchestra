# Notification controls

Place, chat and document bells use the same compact trigger. It names the current effective level,
shows Off on the button and uses the actual state glyph, including when a default turns notifications
off. Pending writes keep the trigger in place; the native acknowledgement supplies its next state.

- `BellTrigger.tsx`: A compact stateful bell: the actual glyph, a visible Off label and the effective level for readers.
- `subscription.ts`: Applies the ordinary subscription reply to its existing page, bell and navigation projections.
