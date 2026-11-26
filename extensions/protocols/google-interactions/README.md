# Google Gemini Interactions

Native dialogue, function tools and Google Search through the Gemini Interactions API.
Orch supplies the admitted conversation with `store=false`, preserves signed native
steps, and requires a terminal event before accepting function calls. Model options
cannot replace history, tools, retention or output bounds. Search modes and options
use the same provider ceiling and controls as the other model interfaces.

The usage reader keeps reported grounding counts, including zero, and leaves missing
counts unknown. Fixtures exercise arbitrary stream byte boundaries without contacting
an upstream or using credentials.

Protocol sources: [Interactions](https://aistudio.google.com/docs/interactions-overview),
[API](https://aistudio.google.com/api-reference/interactions-api) and
[Google Search](https://aistudio.google.com/docs/google-search).
