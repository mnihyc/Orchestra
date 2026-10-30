# Resource facts

Choices, resource names and status items share one set of permitted facts. Hints read list data;
opening a card permits one cached resource-detail read, and live resource use follows the existing
environment feed. Missing measurements stay unknown and never become zero.

| Module | Purpose |
| --- | --- |
| `facts.ts` | Typed resource subjects, applicable cap filters and the shared hint definitions |
| `ResourceFacts.tsx` | Muted hints, named triggers and lazy choice previews |
| `ResourceCard.tsx` | The common frame, core detail bodies and authorized quick actions |
| `ExtraResourceCard.tsx` | Mount, MCP, template, agent-profile and skill detail bodies |
| `ResourceSentence.tsx` | Chips for known resource names inside an existing localized plan sentence |
| `live.ts` | The latest environment resource sample from its existing feed |
