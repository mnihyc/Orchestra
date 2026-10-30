---
title: Add a provider of any kind
screens: /resources/providers, /resources/machines, /thread/$threadId, /topic/$topicId
keywords: provider, subscription, API key, custom endpoint, GPU, adapter, Chat Completions, Responses, Messages, source, 提供方, 订阅, 密钥, 自有模型
---

# Add a provider of any kind

## 1. Choose how the model is reached

Open **Resources → Providers → Add provider**.
Use **Fill from an example** at the top to fill the same fields, or enter your own interface below.
Choose **API address** for a model endpoint, including your own model on a GPU machine.
Give its base address and the API it speaks: Messages, Responses, Chat Completions or Embeddings.
For a subscription, choose **A program's own sign-in** and its sign-in program.
An unavailable program stays in the list with its reason. **Add agent profile** opens the existing
profile flow here; a program installed on a machine still has to report its sign-in support.
You can register it from that machine, or run an unread profile once. The unfinished provider is kept.

## 2. Choose the credential and machine

An address can use **API key** or **No credential**; a subscription uses its program's sign-in.
Choose the provider's home, then continue to the machines and data sources. **Keep the key on**
or **Keep the sign-in on** selects its keeper; without a credential, **Connect from** selects the
machine that reaches the model. Its local policy must allow that address. `localhost` means
that machine, not your browser. **Run the connection on** is a separate choice; both can use
the same machine or the new container.
Only choose a sign-in program on a machine that offers it.

## 3. Choose who reports models and limits

Known vendors have built-in readers. For your own adapter, select a **program on a machine**
or a **source address** for models, limits and usage. A source address implements the provider-source
interface; it is separate from the model's API address. Register a missing program inline or
from the machine's **Programs** section. [Register your program](register-your-program.md)
explains the two kinds. A source reports only what it knows; an absent balance is not zero.

For example, for a GPU model choose its serving address and **Chat Completions**, then a key
or no credential. Select a machine that can reach it and register your source there to list
its models and report its limit. Use that source for the fields it declares; enable the
models you want after creation. No vendor preset is required.

## 4. Create, sign in and use it

Choose **Add provider**. In **Sign-in and keys**, add the key or complete the offered
subscription sign-in. Saving a key also reads its available models and facts. **Refresh** asks again;
accept and enable the models you want in **Models**. Discovery never adds them by itself.
Choose the provider and an enabled model in a thread, then send a small request.
Saved setup is not proof of a working model: a completed turn is.
If the provider is added but a later setup step fails, open the provider and
check its models before finishing setup.

An address refusal belongs beside that field: choose another machine or ask its operator to
allow it. A missing program must be registered on the selected machine. If a sign-in expires,
return to **Sign-in and keys**. Share access only with people who should use that key or plan.
The [source interface guide](/help/provider-readme) explains how to implement your own source.

## Local embeddings

Choose **Local embeddings** in the ordinary example list to run the shipped embedding model in the provider's
runtime. It fills the protocol, loopback address, model and 256 dimensions. The runtime template is an ordinary
Dockerfile template: find it in **Resources → Templates** to edit or remove it. Removing it does not turn it into
a hidden built-in; a later addition makes a fresh template when needed.

Create the provider, then separately save it in the search Embeddings setting. The admin page can open that
editor after creation. Its **Template · Local embeddings** link shows the template actually used.
[Semantic search](semantic-search.md#choose-embeddings) explains accountable use, prefixes and index choices.
