"""Answer synthetic native-control requests without a model, account, HTTP client, or network."""

import json
import sys

answer_file, requests_file, mode, label = sys.argv[1:]
if label == "auth":
    print(json.dumps({"loggedIn": True, "subscriptionType": "max", "email": "ada@example.invalid"}))
    raise SystemExit(0)

for line in sys.stdin:
    frame = json.loads(line)
    with open(requests_file, "a", encoding="utf-8") as recorded:
        recorded.write(json.dumps(frame) + "\n")
    request = frame.get("request", {})
    subtype = request.get("subtype")
    response = {"subtype": "success", "request_id": frame.get("request_id"), "response": {}}
    if subtype == "get_usage":
        if mode == "unanswered":
            continue
        if mode == "refused":
            response = {"subtype": "error", "request_id": frame.get("request_id"), "error": "Unknown control request: get_usage"}
        else:
            with open(answer_file, encoding="utf-8") as source:
                response = json.load(source)["response"]
            response["request_id"] = frame.get("request_id")
    elif subtype != "initialize":
        raise SystemExit("the adapter sent something other than a control observation")
    print(json.dumps({"type": "control_response", "response": response}), flush=True)
