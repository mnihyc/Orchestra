---
title: Pause, switch and remove
screens: /resources/providers/$id, /resources/agents/$id, /resources/environments/$id, /thread/$threadId
keywords: disable, enable, provider, profile, model, environment, stop, switch, queue, delete
---

# Pause, switch and remove

## Disable

Disable pauses a provider, agent profile, model or environment. Running turns stop through the usual interrupt. Each thread records the resource and the person who switched it off. Usage already spent stays recorded.

Waiting messages keep their order and their authors. They stay bound to the resource. Disabling does not hold them as a person's Stop does. The provider or environment units stop after their turns, keeping their volumes and sign-in.

Accepted summary and embedding requests stop too. A stopped request cannot start later just because its resource was enabled again.

The provider, profile and model dialogs accept one optional note. It appears in waiting threads and the stop history.

Open a count to see the affected threads. Names you cannot see stay a count.

## Enable

Enable makes the resource available again. Run the waiting messages starts unchecked. Leave it unchecked to hold those messages for their people to Resume or switch.

Check it to run eligible waiting messages in their original order. Each message keeps its author and passes the normal use checks. A queue held by a person's Stop stays held.

Checking it also continues the latest turns that Disable stopped. Each started turn gets a visible Continue as the original author, before later queued messages. If the turn had not begun, its original input runs instead. If that author can no longer run it, the task stays stopped and its history says why. The person enabling the resource does not become its author.

The count shows messages, threads and when the waiting began. Open it by hovering, focusing or tapping to see the thread names you may view. The last line counts the remaining threads.

Enable on selected resources uses one review and one waiting-message choice. Each row shows its own waiting work. A failed row stays with its result. Retry skips resources already enabled.

## Switch

Switch its threads is separate from Disable and Delete. On a provider, profile or model page or menu, it opens the ordinary batch switch. It selects current bindings the caller may switch. Other threads are counted as skipped.

A source that is switched off can still be left. A target provider, model, profile or environment must be switched on. Messages keep their place, and each author is checked against the new model when their message starts.

## Delete

Deleting a provider or profile stops its turns and keeps its history and usage. Threads can stay bound and wait for a switch. The stored server key is erased and native sign-out is requested. A machine that cannot sign out still needs cleanup there. A pasted vendor credential can still need revocation at its vendor.

The provider dialog counts the affected work and asks for its name. Switch its threads first opens the separate switch. Delete makes no thread choices.

For kept files and other leftovers, use [Storage cleanup](storage-cleanup.md#clean-up-kept-items).
