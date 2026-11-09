---
group: work
title: Delegate a capability to another program
screens: /thread/$threadId, /resources/providers/$id, /usage
keywords: image generation, helper, delegated session, Codex, ChatGPT, subscription, private, sharing
summary: Choose a program sign-in for an image service and follow the private helper that produces its result.
---

# Delegate a capability to another program

A thread can use another program's native capability through an explicitly selected
service. In the thread's capability settings, choose **Service** and its delegated
session backend. For example, a Claude Code thread can ask Codex on an eligible
ChatGPT sign-in to generate an image. Choosing a backend is separate from enabling
the service; an inherited default remains a choice within the place's policies.

The selected provider needs Use and an allowed model. Any sign-in the requesting
person may use can serve the helper, including their own or one shared through
subscription sharing or a Use entry. Orch does not check or enforce a provider's
terms of service. Using a plan this way is the users' responsibility. A missing capability or ineligible plan is shown as unavailable; it does
not silently use another sign-in. A native allowance failure keeps its reported
limit and reset time for the existing call-recovery rules.

Orch creates a short **Helper turn** and links it from the image call. Its owner is
the person on whose behalf the requesting turn runs, including when another
person shared the sign-in. It is an ordinary thread in the requester's place,
private to that actor by default. Its ordinary sharing controls decide who else
may open it. The helper's input links back to the requesting thread.

Everyone who can read the requesting item can see its returned image and helper
link. The label names the program and the helper's private default before opening
it; the reference card checks today's access. Owning the sign-in gives its owner
usage information, not access to the helper's prompt or inputs. Share the helper
normally when its content should be visible to somebody else.

Only the helper's actual completed generation can satisfy the request. A viewed
image, attachment or text answer is not a generated result. The image uses the
ordinary retained image path and is delivered to the files the call requested.
A repeated result or file-delivery retry uses those retained pixels without
starting another paid helper. A failed file write keeps the produced image so
that a different destination can be chosen.

Native use is recorded on the selected provider and helper turn, with the
requester's governing place and image-generation purpose for caps. Linking or
copying the result into the requesting thread adds no generation charge. Missing
model or price observations remain unknown. Cancelling the source stops its owned
initial helper work; later independent work in that ordinary thread is separate.
