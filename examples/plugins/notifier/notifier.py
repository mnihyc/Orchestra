#!/usr/bin/env python3
"""An external notifier for Orch that appends what it is asked to deliver to a file.

A notifier is how Orch tells people outside the application. This one is the smallest that is useful: each
delivery becomes one line of a text file. It shows everything a notifier program has to do and nothing else:
answer `hello`, declare what it is (its descriptor), check the values of a destination when they are saved, and
deliver. It uses nothing but the standard library of Python 3.

Register it in the configuration of the worker, which hosts notifiers:

    [[plugin]]
    kind = "notifier"
    name = "file"
    command = ["/opt/orch-plugins/notifier.py", "--file", "/var/log/orch-notices.log"]

The file is named by the operator who registers the program, not by the people who set up destinations, so that
a destination cannot make the worker write wherever it likes. A destination of this kind has one field: a word
written before each line.

The transport is one JSON object a line on standard input and standard output. This program answers requests one
after another, which the transport allows; a program that does slow work answers them as they finish, in any order.
Standard error is the program's log.
"""

import argparse
import json
import sys

# The versions of the transport and of the notifier contract that this program speaks.
TRANSPORT_VERSION = 1
CONTRACT_VERSION = 1

NAME = "file"
VERSION = "0.1.0"

# What this program implements. `link_url` and `poll_links` are for notifiers that link people to a bot; a
# notifier that does not link anybody leaves them out and is never asked for them.
METHODS = ["describe", "check", "deliver"]

DESCRIPTOR = {
    "name": NAME,
    "title": "Append to a file",
    "summary": "Writes each notification it is told of as one line at the end of a file.",
    # A destination is a place a node is told at. A notifier that reaches people through their own routes says
    # "person" here, and who may set one up in `owners`.
    "targets": ["destination"],
    "fields": [
        {
            "name": "label",
            "label": "Label",
            "type": "text",
            "required": False,
            "help": "A word written before each line, to tell destinations apart.",
            "choices": None,
            "default": None,
            "max_bytes": 40,
        }
    ],
    "owners": [],
    "single": False,
    # The address is the destination's own, and the text names the people it is about.
    "reaches": "channel",
    "body": "text",
    "max_chars": 2000,
    # The texts the notifier ships. `*` serves every language that has no text of its own.
    "templates": {
        "*": {
            "single": {"subject": None, "body": "{{title}}"},
            "digest": {"subject": None, "body": "{{#notifications}}{{title}}\n{{/notifications}}"},
        }
    },
    # Members whose names start with x_ belong to the program: Orch keeps and shows them, and never reads them.
    "x_writes_to": "a file named by the operator",
}


class Notifier:
    def __init__(self, path):
        self.path = path
        # The id of a delivery is the same in every try of it, so a delivery that was written is not written twice.
        self.delivered = set()

    def check(self, params):
        """Looks at the values of a destination when they are saved, and delivers nothing."""
        try:
            with open(self.path, "a", encoding="utf-8"):
                pass
        except OSError as error:
            # Not retryable: the save is refused, and the person sees why.
            raise Refusal("unwritable", "the file cannot be written: %s" % error.strerror)
        return {"file": self.path}

    def deliver(self, params):
        if params["id"] in self.delivered:
            return {}
        values = (params.get("config") or {}).get("values") or {}
        label = values.get("label") or ""
        body = params["body"].replace("\n", " / ")
        line = ("%s: %s" % (label, body)) if label else body
        try:
            with open(self.path, "a", encoding="utf-8") as out:
                out.write(line + "\n")
        except OSError as error:
            # The disk may be full for a moment: try again later.
            raise Refusal("write_failed", str(error), retryable=True)
        self.delivered.add(params["id"])
        return {}


class Refusal(Exception):
    def __init__(self, code, message, retryable=False):
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable


def hello(params):
    """The first exchange: the program picks the highest version that both sides speak."""
    transport = min(params["transport"][1], TRANSPORT_VERSION)
    contract = min(params["contract"][1], CONTRACT_VERSION)
    if transport < params["transport"][0] or contract < params["contract"][0]:
        raise Refusal("unsupported_version", "this program speaks transport %d and contract %d" % (
            TRANSPORT_VERSION, CONTRACT_VERSION))
    return {
        "kind": "notifier",
        "transport": transport,
        "contract": contract,
        "name": NAME,
        "version": VERSION,
        "methods": METHODS,
        "descriptor": DESCRIPTOR,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--file", required=True, help="the file that deliveries are appended to")
    notifier = Notifier(parser.parse_args().file)

    methods = {
        "describe": lambda params: DESCRIPTOR,
        "check": notifier.check,
        "deliver": notifier.deliver,
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
            # Notifications: `cancel` has nothing to stop in a program that does nothing slowly, and `shutdown`
            # asks it to finish and exit. Anything else is not known and is ignored.
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
