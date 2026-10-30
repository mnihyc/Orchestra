#!/usr/bin/env python3
"""An external summarizer for Orch that answers from a fixed rule, with no model.

A summarizer turns what happened in a node into the text of one of its fields. This one needs no model: it
follows three rules, which is as valid as asking one.

    title        the first thing a person said, cut to the limit
    description  the last thing that was said or done, after an optional prefix from its configuration
    tags         the offered tags whose names occur in the activity, at most as many as the limit

It shows everything a summarizer program has to do: answer `hello`, declare what it is, and answer `summarize`.
It uses nothing but the standard library of Python 3.

Register it in the configuration of the API server, which hosts summarizers:

    [[plugin]]
    kind = "summarizer"
    name = "rules"
    command = ["/opt/orch-plugins/summarizer.py"]

and name it in the setting `auto.summarizer`: `{ "kind": "rules", "prefix": "Now: " }`. Everything in that setting
but `kind` arrives as `config`, after it was checked against the schema below.

The transport is one JSON object a line on standard input and standard output. Standard error is the program's log.
"""

import json
import sys

# The versions of the transport and of the summarizer contract that this program speaks.
TRANSPORT_VERSION = 1
CONTRACT_VERSION = 1

NAME = "rules"
VERSION = "0.1.0"

METHODS = ["describe", "summarize"]

DESCRIPTOR = {
    "name": NAME,
    "title": "By rules",
    "config_schema": {
        "type": "object",
        "properties": {
            "prefix": {"type": "string", "description": "Written before every description."},
        },
        "additionalProperties": False,
    },
    # The fields of a node that this summarizer can write. One that is not listed is never asked for.
    "fields": ["title", "description", "tags"],
    # Members whose names start with x_ belong to the program: Orch keeps and shows them, and never reads them.
    "x_rules": ["first words of the person", "last thing said", "tags that occur"],
}


class Refusal(Exception):
    def __init__(self, code, message, retryable=False):
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable


def cut(text, characters):
    """At most `characters` characters, never inside one."""
    return text[:characters] if characters > 0 else text


def first_line(text):
    for line in text.splitlines():
        if line.strip():
            return line.strip()
    return ""


def summarize(params):
    field = params["field"]
    activity = params.get("activity") or []
    limits = params.get("limits") or {}
    config = params.get("config") or {}
    if not activity:
        # Nothing happened since the last summary. That is no failure: the setting says why it was skipped.
        raise Refusal("skipped", "nothing happened since the last summary")

    if field == "title":
        said = [item["text"] for item in activity if item["kind"] == "user_message"]
        source = said[0] if said else activity[0]["text"]
        return {"text": cut(first_line(source), limits.get("max_chars", 80)), "tags": None, "usage": None}

    if field == "description":
        text = config.get("prefix", "") + first_line(activity[-1]["text"])
        return {"text": cut(text, limits.get("max_chars", 200)), "tags": None, "usage": None}

    if field == "tags":
        spoken = " ".join(item["text"] for item in activity).lower()
        found = [tag for tag in params.get("tag_choices") or [] if tag.lower() in spoken]
        # An empty list is an answer too: none of the tags applies. `usage` is null: no model was called.
        return {"text": None, "tags": found[: limits.get("max_tags", 0)], "usage": None}

    raise Refusal("unsupported", "this summarizer does not write the field %r" % field)


def hello(params):
    """The first exchange: the program picks the highest version that both sides speak."""
    transport = min(params["transport"][1], TRANSPORT_VERSION)
    contract = min(params["contract"][1], CONTRACT_VERSION)
    if transport < params["transport"][0] or contract < params["contract"][0]:
        raise Refusal("unsupported_version", "this program speaks transport %d and contract %d" % (
            TRANSPORT_VERSION, CONTRACT_VERSION))
    return {
        "kind": "summarizer",
        "transport": transport,
        "contract": contract,
        "name": NAME,
        "version": VERSION,
        "methods": METHODS,
        "descriptor": DESCRIPTOR,
    }


def main():
    methods = {
        "describe": lambda params: DESCRIPTOR,
        "summarize": summarize,
        "ping": lambda params: {},
    }
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            request = json.loads(raw)
        except ValueError:
            continue
        name = request.get("method")
        if "id" not in request:
            # `cancel` has nothing to stop in a program that answers at once; `shutdown` asks it to exit.
            if name == "shutdown":
                return
            continue
        try:
            if name == "hello":
                result = hello(request.get("params") or {})
            elif name in methods:
                result = methods[name](request.get("params") or {})
            else:
                # A method that is not known is answered with `unsupported`, and the program goes on.
                raise Refusal("unsupported", "unknown method %r" % name)
            reply = {"id": request["id"], "result": result}
        except Refusal as refusal:
            reply = {"id": request["id"], "error": {
                "code": refusal.code, "message": refusal.message, "retryable": refusal.retryable}}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
