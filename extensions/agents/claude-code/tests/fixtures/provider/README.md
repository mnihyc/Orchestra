# Native protocol test support

`harness.py` answers scripted native-control requests without a model, account, HTTP client, or network.
The quota inputs are constructed explicitly in `tests/plan_facts.rs`; they are not captured account replies.

The setup exercises the experimental `get_usage` control shape used by the pinned 2.1.296 build and
its turn notifications. Keep parsing defensive: an unanswered or changed reply is an unread fact,
while known members of a partial answer remain useful.

Window use, model-family labels, plan names, and credit amounts are independent fixed test examples.
Reset times come from the deterministic test clock plus each window's declared duration, never from
account observations. The control and turn examples retain distinct percentage/fraction encodings.

The helper writes only the adapter's requests to a temporary log, so tests can require observation-only
control requests and verify that no model turn, account credential, or vendor HTTP request is used.
