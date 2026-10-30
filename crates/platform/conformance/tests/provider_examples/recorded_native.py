"""A named native-harness double: recorded JSON answers only, no account or network."""
import argparse
import copy
import json
from pathlib import Path
import sys

parser = argparse.ArgumentParser()
parser.add_argument("--tape", required=True)
parser.add_argument("--state", required=True)
args, native_args = parser.parse_known_args()
tape = json.loads(Path(args.tape).read_text())
state = Path(args.state)

def emit(value):
    print(json.dumps(value), flush=True)

if "debug" in native_args:
    emit(tape.get("catalogue", {}))
    sys.exit(0)

for line in sys.stdin:
    frame = json.loads(line)
    stream = frame.get("type") == "control_request"
    method = frame.get("request", {}).get("subtype") if stream else frame.get("method")
    identifier = frame.get("request_id") if stream else frame.get("id")
    if identifier is None:
        continue
    with state.with_suffix(".requests").open("a") as requests:
        requests.write(json.dumps(frame) + "\n")
    answer = {}
    if method == "initialize":
        answer = tape.get("initialize", {})
    elif method == "getAuthStatus":
        answer = {"authMethod": "chatgpt"}
    elif method == "account/read":
        answer = tape.get("account", {})
    elif method == "account/rateLimits/read":
        for observation in tape.get("notifications", []):
            emit(observation)
        answer = copy.deepcopy(tape.get("rates", {}))
        if state.exists():
            answer.get("rateLimitResetCredits", {})["availableCount"] = 0
            for credit in answer.get("rateLimitResetCredits", {}).get("credits") or []:
                credit["status"] = "redeemed"
    elif method == "account/rateLimitResetCredit/consume":
        params = frame["params"]
        previous = json.loads(state.read_text()) if state.exists() else None
        if previous and previous["idempotencyKey"] != params["idempotencyKey"]:
            emit({"id": identifier, "error": {"code": -1, "message": "no disposable credit remains"}})
            continue
        if not previous:
            state.write_text(json.dumps({**params, "spent": 1}))
        answer = {"outcome": "reset"}
    elif method == "model/list":
        answer = tape.get("models", {"data": [], "nextCursor": None})
    elif method == "get_usage":
        for observation in tape.get("notifications", []):
            emit(observation)
        if tape.get("usage_error"):
            emit({"type": "control_response", "response": {"subtype": "error", "request_id": identifier, "error": "unsupported"}})
            continue
        answer = tape.get("usage", {})
    if stream:
        emit({"type": "control_response", "response": {"subtype": "success", "request_id": identifier, "response": answer}})
    else:
        emit({"id": identifier, "result": answer})
