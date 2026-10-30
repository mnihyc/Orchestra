#!/usr/bin/env python3
"""A plugin of the Orch plugin transport whose behaviour is given by a script and by its arguments.

It is the other end of the host's tests: it speaks the transport (one JSON object a line, on standard input and
output or on a socket), answers each method from a script, and, when asked to by an argument, misbehaves in one of
the ways a real plugin does. It uses nothing but the standard library of Python 3.

The script is a JSON file given with --script:

    {
      "kind": "driver", "name": "scripted", "version": "0.1.0",
      "methods": ["describe", "create", ...],          // what hello declares
      "descriptor": { ... },                           // the kind's descriptor
      "responses": {                                   // by method; a list is used in turn, the last one repeats
        "create": { "result": { ... } },               // or "error": { "code": ..., "message": ..., "retryable": ... }
                                                       // "delay_ms": N waits (and ends early when cancelled)
                                                       // "progress": [ { "message": ..., "done": 1, "total": 3 } ]
                                                       // "events": [ { "event": "...", "params": { ... } } ] before the
                                                       // answer, "events_after" after it
      }
    }

In a result, { "$params": "a.b" } is replaced by the member a.b of the request's parameters, and
{ "$upper": X } by X in upper case.

Every line received and every start is appended to the file given with --log, as one JSON object a line.
A method that has no response in the script is answered with the error "unsupported".
"""

import argparse
import json
import os
import socket
import sys
import threading
import time

ARGS = None
SCRIPT = {}
LOG_LOCK = threading.Lock()


def log(record):
    if not ARGS.log:
        return
    with LOG_LOCK:
        with open(ARGS.log, "a", encoding="utf-8") as out:
            out.write(json.dumps(record) + "\n")


def starts_so_far():
    """How many times this plugin was started, this start included: the starts the log has recorded."""
    if not ARGS.log or not os.path.exists(ARGS.log):
        return 0
    with open(ARGS.log, encoding="utf-8") as seen:
        return sum(1 for line in seen if '"started"' in line)


def lookup(value, path):
    for part in path.split("."):
        if isinstance(value, dict) and part in value:
            value = value[part]
        else:
            return None
    return value


def expand(template, params):
    if isinstance(template, dict):
        if set(template) == {"$params"}:
            return lookup(params, template["$params"])
        if set(template) == {"$upper"}:
            inner = expand(template["$upper"], params)
            return inner.upper() if isinstance(inner, str) else inner
        return {key: expand(value, params) for key, value in template.items()}
    if isinstance(template, list):
        return [expand(item, params) for item in template]
    return template


class Session:
    """One conversation: standard streams, or one accepted connection."""

    def __init__(self, reader, writer, closer=None):
        self.reader = reader
        self.writer = writer
        self.closer = closer
        self.lock = threading.Lock()
        self.cancelled = {}
        self.turns = {}
        self.held = []
        self.hold_timer = None
        self.dead = False

    def send(self, message):
        line = json.dumps(message)
        with self.lock:
            if self.dead:
                return
            try:
                self.writer.write(line + "\n")
                self.writer.flush()
            except (OSError, ValueError):
                self.dead = True

    def raw(self, text):
        with self.lock:
            try:
                self.writer.write(text + "\n")
                self.writer.flush()
            except (OSError, ValueError):
                self.dead = True

    def garbage(self, count):
        samples = ["this is not json", "[1, 2, 3]", '{"hello": 1}', '{"id": "one", "result": {}}']
        for index in range(count):
            self.raw(samples[index % len(samples)])

    def extras(self, message):
        if ARGS.extras:
            message["served_by"] = "scripted"
            message["x_trace"] = {"n": 1}
        return message

    def finish_connection(self):
        self.dead = True
        if self.closer:
            self.closer()

    def hello(self, request):
        params = request.get("params") or {}
        offered_transport = params.get("transport", [1, 1])
        offered_contract = params.get("contract", [1, 1])
        result = {
            "kind": ARGS.hello_kind or SCRIPT.get("kind", "summarizer"),
            "transport": ARGS.hello_transport if ARGS.hello_transport is not None else offered_transport[1],
            "contract": ARGS.hello_contract if ARGS.hello_contract is not None else offered_contract[1],
            "name": SCRIPT.get("name", "scripted"),
            "version": SCRIPT.get("version", "0.1.0"),
            "methods": SCRIPT.get("methods", []),
            "descriptor": SCRIPT.get("descriptor", {}),
        }
        if ARGS.extras:
            result["added_by_a_later_version"] = {"anything": True}
            result["methods"] = result["methods"] + ["x_snapshot"]
            if isinstance(result["descriptor"], dict):
                result["descriptor"] = dict(result["descriptor"], x_note="kept", from_the_future=1)
        if ARGS.garbage_hello:
            self.garbage(ARGS.garbage_hello)
        self.send(self.extras({"id": request["id"], "result": result}))
        if ARGS.close_output_after_hello:
            # sys.stdout does not close the descriptor it wraps; the descriptor is what the host sees.
            sys.stdout.flush()
            os.close(1)
        if ARGS.exit_after_hello or (ARGS.exit_after_hello_until and starts_so_far() <= ARGS.exit_after_hello_until):
            self.finish_exit(ARGS.exit_code)
        if ARGS.hang_after_hello:
            while True:
                time.sleep(3600)

    def finish_exit(self, code):
        if ARGS.listen:
            self.finish_connection()
            raise ConnectionAbort()
        sys.stdout.flush()
        os._exit(code)

    def response_for(self, method):
        entry = SCRIPT.get("responses", {}).get(method)
        if entry is None:
            return None
        if isinstance(entry, list):
            with LOG_LOCK:
                index = self.turns.get(method, 0)
                self.turns[method] = index + 1
            return entry[min(index, len(entry) - 1)]
        return entry

    def answer(self, request):
        request_id = request["id"]
        method = request["method"]
        params = request.get("params")
        if method == "ping":
            if not ARGS.no_pong:
                self.send({"id": request_id, "result": {}})
            return
        if method == ARGS.hang_on:
            return
        if method == ARGS.exit_on and self.should_exit_once():
            self.finish_exit(ARGS.exit_code)
            return
        entry = self.response_for(method)
        if entry is None:
            self.finish_reply(request_id, {"error": {"code": "unsupported",
                                                     "message": "unknown method %r" % method,
                                                     "retryable": False}})
            return
        for progress in entry.get("progress", []):
            self.send({"progress": request_id, "params": progress})
        for event in entry.get("events", []):
            self.send({"event": event["event"], "params": expand(event.get("params", {}), params)})
        delay = entry.get("delay_ms", 0) / 1000.0
        if delay:
            cancelled = self.cancelled.setdefault(request_id, threading.Event())
            if cancelled.wait(delay):
                self.send({"id": request_id, "error": {"code": "cancelled", "message": "cancelled",
                                                       "retryable": False}})
                return
        reply = {}
        if "error" in entry:
            reply["error"] = entry["error"]
        else:
            reply["result"] = expand(entry.get("result", {}), params)
        self.finish_reply(request_id, reply)
        for event in entry.get("events_after", []):
            self.send({"event": event["event"], "params": expand(event.get("params", {}), params)})

    def should_exit_once(self):
        if not ARGS.once_marker:
            return True
        if os.path.exists(ARGS.once_marker):
            return False
        with open(ARGS.once_marker, "w", encoding="utf-8") as marker:
            marker.write("exited once\n")
        return True

    def finish_reply(self, request_id, reply):
        message = dict(reply, id=request_id)
        if ARGS.out_of_order:
            self.hold(message)
            return
        self.deliver(message)

    def deliver(self, message):
        if ARGS.garbage:
            self.garbage(ARGS.garbage)
        self.send(self.extras(message))

    def hold(self, message):
        """Answers in the opposite order to the requests: held until as many as --out-of-order are, then released
        backwards. A safety timer releases what is held if the rest never comes."""
        with self.lock:
            self.held.append(message)
            full = len(self.held) >= ARGS.out_of_order
            if full:
                if self.hold_timer:
                    self.hold_timer.cancel()
            elif self.hold_timer is None:
                self.hold_timer = threading.Timer(10.0, self.release)
                self.hold_timer.daemon = True
                self.hold_timer.start()
        if full:
            self.release()

    def release(self):
        with self.lock:
            held, self.held = self.held, []
            self.hold_timer = None
        # By the number of the request, highest first, however the threads that held them were scheduled.
        for message in sorted(held, key=lambda held_message: held_message["id"], reverse=True):
            self.deliver(message)

    def handle(self, request):
        method = request.get("method")
        if method == "cancel":
            target = (request.get("params") or {}).get("id")
            self.cancelled.setdefault(target, threading.Event()).set()
            return
        if method == "shutdown":
            if ARGS.shutdown_ignore:
                return
            self.finish_exit(ARGS.shutdown_code)
            return
        if "id" not in request:
            return
        if method == "hello":
            if ARGS.silent_hello:
                return
            self.hello(request)
            return
        threading.Thread(target=self.guarded, args=(request,), daemon=True).start()

    def guarded(self, request):
        try:
            self.answer(request)
        except ConnectionAbort:
            pass

    def run(self):
        try:
            for raw in self.reader:
                raw = raw.strip()
                if not raw:
                    continue
                try:
                    request = json.loads(raw)
                except ValueError:
                    log({"received": raw})
                    continue
                log({"received": request})
                try:
                    self.handle(request)
                except ConnectionAbort:
                    return
        except (OSError, ValueError):
            pass
        if not ARGS.listen:
            while ARGS.shutdown_ignore:
                time.sleep(3600)
            sys.stdout.flush()
            os._exit(0)


class ConnectionAbort(Exception):
    pass


def flood_stderr():
    line = "flooding the log with a line that says nothing of any use " * 4
    while True:
        sys.stderr.write(line + "\n")
        sys.stderr.flush()


def serve_socket(address):
    if address.startswith("unix:"):
        path = address[len("unix:"):]
        if os.path.exists(path):
            os.unlink(path)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        ready = {"path": path}
    else:
        port = int(address.split(":")[-1])
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", port))
        ready = {"port": server.getsockname()[1]}
    server.listen(4)
    if ARGS.ready:
        # Written whole and put in place in one step, so that whoever waits for the file never reads half of it.
        with open(ARGS.ready + ".part", "w", encoding="utf-8") as out:
            out.write(json.dumps(ready))
        os.replace(ARGS.ready + ".part", ARGS.ready)
    while True:
        connection, _ = server.accept()
        log({"accepted": True, "pid": os.getpid()})
        reader = connection.makefile("r", encoding="utf-8", newline="\n")
        writer = connection.makefile("w", encoding="utf-8", newline="\n")

        def closer(connection=connection):
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()

        Session(reader, writer, closer).run()
        closer()


def main():
    global ARGS, SCRIPT
    parser = argparse.ArgumentParser()
    parser.add_argument("--script")
    parser.add_argument("--log")
    parser.add_argument("--listen")
    parser.add_argument("--ready")
    parser.add_argument("--hello-transport", type=int)
    parser.add_argument("--hello-contract", type=int)
    parser.add_argument("--hello-kind")
    parser.add_argument("--hang-on")
    parser.add_argument("--hang-after-hello", action="store_true")
    parser.add_argument("--silent-hello", action="store_true")
    parser.add_argument("--close-output-after-hello", action="store_true")
    parser.add_argument("--no-pong", action="store_true")
    parser.add_argument("--exit-on")
    parser.add_argument("--exit-after-hello", action="store_true")
    parser.add_argument("--exit-after-hello-until", type=int, default=0)
    parser.add_argument("--exit-code", type=int, default=3)
    parser.add_argument("--once-marker")
    parser.add_argument("--garbage", type=int, default=0)
    parser.add_argument("--garbage-hello", type=int, default=0)
    parser.add_argument("--out-of-order", type=int, default=0)
    parser.add_argument("--extras", action="store_true")
    parser.add_argument("--flood-stderr", action="store_true")
    parser.add_argument("--stderr-line")
    parser.add_argument("--shutdown-ignore", action="store_true")
    parser.add_argument("--shutdown-code", type=int, default=0)
    ARGS = parser.parse_args()
    if ARGS.script:
        with open(ARGS.script, encoding="utf-8") as script:
            SCRIPT = json.load(script)
    log({"started": os.getpid(), "at": time.time()})
    if ARGS.stderr_line:
        sys.stderr.write(ARGS.stderr_line + "\n")
        sys.stderr.flush()
    if ARGS.flood_stderr:
        threading.Thread(target=flood_stderr, daemon=True).start()
    if ARGS.listen:
        serve_socket(ARGS.listen)
    else:
        Session(sys.stdin, sys.stdout).run()


if __name__ == "__main__":
    main()
