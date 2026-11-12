---
group: resources
screens: [/resources/machines/$id, /resources/mcp, /resources/mcp/$id, /resources/environments/$id, /thread/$threadId]
keywords: [MCP, import, Codex, Claude Code, tools, switch on, machine, environment, Forget, policy]
summary: Import machine-local MCP definitions as resources and enable them where they are needed.
---

# Import and switch on MCP servers

Import makes a server from an agent program's own machine configuration visible as an Orch resource. Its secret-bearing definition stays on that machine. Switching it on gives it to the chosen environment or place through the existing MCP setting.

## Where

Open a machine from [Machines](/resources/machines), then Tools. **Found in the agents' own configuration** shows the reported program, scope, transport and import state. Import reviews the name and definition's shape, with **Switch on here** selected initially. Use an alias when the original name does not fit.

The same flow is at [MCP servers](/resources/mcp) → Add MCP server → Import from a machine. Choose an environment you may use. In a thread's Environment panel or an environment's Tools, **Import and switch on here** can handle reported configurations in one action per origin. The action remains subject to machine policy and import authority.

## Example

Workstation reports a Codex server called docs. Import it into the Research environment and leave Switch on here selected. Open the server to inspect **Switched on in**, or choose another named place there. In environment Tools, use the server's switch to turn use off or on without typing an identifier.

## What to expect

The server reports Starting, Running, Stopped, Failed or Unavailable with its reason. The list says when an update applies: immediately, next turn or next session. A copy imported from Workstation cannot use another machine's same-named secrets; it is unavailable there until you deliberately provide an appropriate server.

An accepted import may show Imported before a restricted view can display its resource link. A partially completed set keeps completed origins and offers the remaining ones. Policy-off discovery is not a claim that the program has no servers. **Forget** removes the imported Orch resource; it does not erase the program's own configuration. See [resource groups](resource-groups.md) and [environment switching](switch-and-move-environments.md).
