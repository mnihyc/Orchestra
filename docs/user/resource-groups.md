---
group: resources
screens: [/resources, /everything, /resources/environments, /resources/hosts, /resources/mcp]
keywords: [resources, group by, home, owner, machine, origin, tags, picker]
summary: Group resource views by home, owner, machine or tag without changing access.
---

# Organize resources by home and origin

Resource groups are views of existing resource facts: Home, Owner, Machine and Tag. A display group does not create another resource or grant access. A resource's home holds its sharing policy; its machine of origin says where it came from or can run. A container's machine is named by its host: a resource kept in one says **From the container on** and the host's name, and the containers of one host stand in one group.

## Where

Open [Resources](/resources) and a resource list. Beside Sort, choose **Group by → Home / Owner / Machine / Tag / None**. Home is the initial grouping; each list remembers its own last choice on this device. [Everything](/everything) also places resources under their homes.

Resource selectors group options by readable home paths, with the current place's group first. Type to narrow across groups. Arrow keys move through options, skipping headings; Enter chooses an eligible option. Inspect an unavailable option's reason before selecting a different one.

## Example

Keep hosts A and B in the Guests project and give the intended people Use through that project's sharing policy. Group the [host list](/resources/hosts) by Home to see them together. Separately, group [MCP servers](/resources/mcp) by Machine to find the copies imported from Workstation.

## What to expect

A resource imported from or created on a machine initially follows that machine's home. A manager can use **Move to another home** to override it; later imports or registrations keep that deliberate choice.

Changing the display grouping does not move a resource. Moving its home does not move its files or change machine locality. An imported MCP definition stays bound to its own machine even when its resource has another home. Read [MCP import and use](mcp-import-and-use.md) or [switching and moving environments](switch-and-move-environments.md) for those actions.
