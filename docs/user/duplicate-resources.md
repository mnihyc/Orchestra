---
group: resources
screens: [/documents/$nodeId, /document-folder/$folderId, /resources/hosts, /resources/hosts/$id, /resources/machines, /resources/machines/$id, /resources/providers, /resources/providers/$id, /resources/agents, /resources/agents/$id, /resources/templates, /resources/templates/$id, /resources/environments, /resources/environments/$id, /resources/mcp, /resources/mcp/$id, /resources/mounts, /resources/mounts/$id, /resources/skills, /resources/skills/$id, /doc/$documentId, /thread/$threadId, /org/$slug/messages, /chat/$chatId]
keywords: [duplicate, copy, destination, home, files, credentials, group chat, fork]
summary: Make an independent copy of a resource or a conversation, choose where it belongs, and move it to another home or organization.
---

# Duplicate a resource or conversation

Open the **⋯** menu of the thing, or right-click its row.

- **Duplicate** starts a copy in the same place, with a new name that the service chooses, and opens the new thing when it is accepted.
- **Duplicate to…** opens the shared place picker across your organizations. Choose a destination and, if you like, a name.
- Cancelling sends no request to copy.

## What follows

The copy takes the defaults and the locks of the destination and a new permission page. The shares, the comments, the usage and the administrative history that exist stay with the original.

- An environment copy takes its workspace that you can read into new storage that it owns.
- A document copy takes the content and the files that you can read.
- A skill's usual action copies its published version. **Duplicate draft**, in its draft editor, copies the draft that is not published, through a separate operation.
- **Duplicate** of a thread is its fork. The review of access of its agent and the plan of its job still apply. **Duplicate to…** lets you choose a topic in the same place picker, and the plan checks the destination and the resources it would use. Threads of the built-in programs keep their own flow for a private creation.

The person who starts a thread move can still inspect its accepted job after the original is deleted. A scoped credential keeps the job’s actual organization and place restrictions.

## Credentials and notices

The operation of the resource decides whether you own the source and may carry its secrets. Copies for other readers need their own key. A copy of a subscription starts signed out, with a sign-in of its own. Copying never asks the browser to fetch and send again the credentials of the source.

For a definition of an MCP server that was imported, **Machine** chooses the machine that receives it, apart from the home where it is shared. Leaving **Same machine** keeps the original recipient if it can be used at the destination. A refusal asks for a machine that is eligible. Choosing another home gives no permission on a machine.

A copy of a group chat starts with a new permission page and a roster containing only its creator. Copying does not notify the original group.

## When a destination is refused

Reading the source and creating that kind of thing at the destination are separate checks. Taking a document, a chat or a thread into another organization also needs its **Export** permission, and the policy of the source, **Copies to other orgs**, must allow it. That policy follows the settings and locks of the system, the organization and the project.

- The place picker keeps destinations that are unavailable visible, with the real cause. Its requirements button opens the details of access, or the help of the setting of the policy of the source.
- Some places need a placement, a machine or a plan for the thing itself before copying can go on.
- A request that was refused keeps the place you selected and shows the problem with access or with a field. Review the access that is missing, or choose another destination.
- An acknowledgement that is accepted opens the new thing. Opening it can still report that the new thing is unavailable to this reader.
- Retrying a lost answer without changing your choices repeats the same act and does not make another copy.

## Move a home within its org

**Move…** uses the same place picker to change the home of a resource within its root organization. The departments of an organization count as places within that root. The operation needs permission to manage the source and the destination. It keeps the identity and the files, and uses the new home for inheritance. A group uses the same choice through its own update. Direct chats offer no **Move**.

Hosts and machines offer this change of home without **Duplicate**. For an environment, **Move to another machine…** stays its separate plan for moving it physically. Changing a home does not do that.

## Move to another org

In **Move…**, choosing a home in another root organization changes the button to **Copy to <org>, then delete**. Only an owner of the source may use it. Every rule for copying out and for deleting still applies. A manager or an administrator who is not an owner cannot use this move.

For providers, agents, skills, mounts, templates, hosts and machines, review the **Deletion plan** before confirming. Its rows show the actual consequences, including stopping bound work or keeping a registration. The copy and source removal happen in one operation; cancelling the dialog changes neither. Hosts and machines keep their registration and identifier rather than creating another registration. Environment moves across orgs remain unavailable until their native move operation is ready.

After a refused move, the place and reason stay visible. If the source or deletion footprint changed, **Review again** loads a new plan for you to confirm. Retrying a lost response keeps the original operation and cannot make a second copy. The accepted identity opens through its ordinary reader.


Documents and document-only folders use the same **Move…** door. Choose the org or project, and optionally a folder in that org. A move within the org keeps its identity; one into another org reviews the complete native deletion membership and retention deadline before copying and removing the source in one operation. A protected preview says **Deletion plan ready** without exposing its hidden membership. After acceptance, a mapped copied root opens directly; a sparse folder acknowledgement without that map opens the chosen destination directory.


An MCP move uses its native source-deletion plan. For an imported definition, **Machine** chooses the destination custodian separately from the sharing home. **Same machine** keeps the current recipient only if the native checks permit it; choosing another machine refreshes the deletion footprint before confirmation. An owned definition keeps its class and has no imported-machine choice.

For a group, the native operation checks its actual current managers and copy-out rules. Review **End this group and its history**, type the shown group name, then confirm the single move. **Notify N people** uses the original group's native member preview; it grants no access to the new creator-only group. Leaving it off permits a silent move. If the reviewed group or audience changes, **Review again** clears the name confirmation and notice choice for a fresh decision.


Moving a provider uses the native creation and successful source-deletion path. Its stop and erasure consequences appear in the plan. A subscription sign-in is handed over in that same act; ordinary Duplicate still starts its subscription copy signed out. The browser never reads and resubmits those credentials.


For a coding thread, open its **⋯** menu and choose **Move to…**. Select a topic in another organization. If a binding is unavailable there, choose its replacement in the plan. Review the full conversation copy, fresh destination access and original deletion together. Only an actual owner can confirm; Export, copy-out policy, destination creation and original deletion checks still apply. Open side threads must be closed or kept first, as named in the plan. The native job deletes the original only after the copy commits, and can be hidden or cancelled before commit. Its progress remains readable after the original disappears; **Open moved thread** opens the accepted new conversation. **Notify N people** is optional and follows the reviewed contributor set. Built-in program conversations use the same Move entry and original deletion review. They preserve their program, copy all history, use destination access defaults and search/govern in the destination organization, with no coding profile or environment. The copy and original ending commit together; Retry after a lost reply acknowledges the same result.
