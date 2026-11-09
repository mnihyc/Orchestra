---
group: resources
screens: [/documents/$nodeId, /document-folder/$folderId, /resources/hosts, /resources/hosts/$id, /resources/machines, /resources/machines/$id, /resources/providers, /resources/providers/$id, /resources/agents, /resources/agents/$id, /resources/templates, /resources/templates/$id, /resources/environments, /resources/environments/$id, /resources/mcp, /resources/mcp/$id, /resources/mounts, /resources/mounts/$id, /resources/skills, /resources/skills/$id, /doc/$documentId, /thread/$threadId, /org/$slug/messages, /chat/$chatId]
keywords: [duplicate, copy, destination, home, files, credentials, group chat, fork]
summary: Make an independent copy through the source's native operation and choose where it belongs.
---

# Duplicate a resource or conversation

Open the thing’s **⋯** menu or right-click its row. **Duplicate** starts a copy in the same place, using a fresh name chosen by the service, and opens the new identity when accepted. **Duplicate to…** opens the common place picker across your orgs; choose a destination and optionally a name. Cancelling sends no copy request.

## What follows

The copy takes the destination’s defaults, locks and a fresh permission page. Existing shares, comments, usage and administrative history stay with the original. An environment copy takes its readable workspace into fresh owned storage; a document copy takes its readable content and files. A skill’s ordinary action copies its published version. **Duplicate draft**, in its draft editor, copies the unpublished draft through that separate operation.

A thread’s Duplicate is its native fork. Its existing agent-access review and job plan still apply. Duplicate to lets you choose a topic using the same place picker; the plan checks the destination and the resources it would use. Built-in program threads retain their own Private creation flow.

## Credentials and notices

The native operation decides whether you own the source and may carry its secrets. Other readers’ copies need their own key. Subscription copies start signed out, with an independent sign-in. Copying never asks the browser to retrieve and resubmit source credentials.

For an imported MCP definition, **Machine** chooses its physical recipient separately from its sharing home. Leaving **Same machine** keeps the original recipient if it is usable at the destination. A refusal asks for an eligible machine; choosing another home grants no machine permission.

A group-chat copy starts with a new permission page and creator-only roster. Duplicate to offers **Notify N people** from the source’s native contributor preview. Leave it unticked for a silent copy. A failed preview or a count over its limit leaves notification unavailable while a silent copy is still possible. Notices give nobody access to the new conversation.

## When a destination is refused

Reading the source and creating that kind at the destination are independent checks. Taking a document, chat or thread into another org also needs its Export permission, and the source’s **Copies to other orgs** policy must permit it. The policy follows its system, org and project settings and locks.

The place picker keeps unavailable destinations visible with their actual cause. Its requirements button opens the access details or the source policy’s setting help. Some places need a placement, a machine or a native plan before copying can proceed. A refused request keeps the selected place and shows the native access or field problem. Review the missing access or choose another destination. An accepted sparse acknowledgement opens its actual new identity; the ordinary opening can still report that the new thing is unavailable to this reader. Retrying a lost answer without changing choices repeats the same act and does not make another copy.

## Move a home within its org

**Move…** uses the same place picker to change a resource’s home within its root org. Org departments count as places within that root. The native operation requires management of the source and destination; it keeps the identity and files, and uses the new home for inheritance. A group uses the same choice through its own group update. Direct chats offer no Move.

Hosts and machines offer this home move without Duplicate. For an environment, **Move to another machine…** remains its separate physical migration plan. Changing a home does not perform that migration.


## Move to another org

In **Move…**, choosing a home in another root org changes the button to **Copy to <org> and delete here**. Only an owner of the source may use it. Every copy-out and deletion rule still applies; a manager or administrator who is not an owner cannot use this move.

For providers, agents, skills, mounts, templates, hosts and machines, review the **Deletion plan** before confirming. Its rows show the actual consequences, including stopping bound work or keeping a registration. The copy and source removal happen in one operation; cancelling the dialog changes neither. Hosts and machines keep their registration and identifier rather than creating another registration. Environments and threads remain unavailable until their move operation is ready.

After a refused move, the place and reason stay visible. If the source or deletion footprint changed, **Review again** loads a new plan for you to confirm. Retrying a lost response keeps the original operation and cannot make a second copy. The accepted identity opens through its ordinary reader.


Documents and document-only folders use the same **Move…** door. Choose the org or project, and optionally a folder in that org. A move within the org keeps its identity; one into another org reviews the complete native deletion membership and retention deadline before copying and removing the source in one operation. A protected preview says **Deletion plan ready** without exposing its hidden membership. After acceptance, a mapped copied root opens directly; a sparse folder acknowledgement without that map opens the chosen destination directory.


An MCP move uses its native source-deletion plan. For an imported definition, **Machine** chooses the destination custodian separately from the sharing home. **Same machine** keeps the current recipient only if the native checks permit it; choosing another machine refreshes the deletion footprint before confirmation. An owned definition keeps its class and has no imported-machine choice.

For a group, the native operation checks its actual current managers and copy-out rules. Review **End this group and its history**, type the shown group name, then confirm the single move. **Notify N people** uses the original group's native member preview; it grants no access to the new creator-only group. Leaving it off permits a silent move. If the reviewed group or audience changes, **Review again** clears the name confirmation and notice choice for a fresh decision.


Moving a provider uses the native creation and successful source-deletion path. Its stop and erasure consequences appear in the plan. A subscription sign-in is handed over in that same act; ordinary Duplicate still starts its subscription copy signed out. The browser never reads and resubmits those credentials.
