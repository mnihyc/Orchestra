# Captured native image generation

These turns were recorded from Codex CLI 0.167.0 `app-server` in an isolated,
throwaway harness container. The success used an authorized ChatGPT sign-in;
the refusal used an empty program home. The account home and container were
removed. Tests replay data only and never open a model connection or an account.

The input requested one small red circle on white, saved as `red-circle.png`.
The thread used `/work`, `approvalPolicy: on-request` and `sandbox: workspace-write`.
No server request was accepted; none arrived. The one configuration warning
comes from the container's workspace sandbox setup.

Rows preserve arrival order and use `direction` plus the JSON-RPC `frame`.
Each distinct long token has one stable `redacted-NNN` stand-in; the address is
`person@example.invalid`, account/installation identities are fixed stand-ins,
and the server is `recorder-host`. Native thread, turn, generation and request
correlation remains recorded. The bundled image-generation skill text inside a
command's output is covered by the extension's existing upstream notices.

The successful native image's saved path and inline result both use
`@fixture/generated.png`. The unchanged PNG is 1,065,038 bytes; the native inline
bytes matched that saved file. Replay materializes only those explicit aliases
inside its owned temporary directory. The later recorded `cp` is displayed as
data and never executed by the test.

Four success-stream account-usage notifications are replaced by explicit
`{"direction":"from_harness","redacted":"account_usage"}` rows. Those rows keep
their positions and claim no protocol event or measured value. Personal plan
usage and reset facts are not shipped. Image, authentication, token-usage and
turn evidence is otherwise retained. `SHA256SUMS` covers both frame files and the
unchanged PNG after this final redaction.

The empty-home capture has ten retrying 401 errors, then a failed turn carrying
`httpConnectionFailed` with status 401. This is a real sign-in refusal. The limit
and reset test is separately named **schema-derived**: no exhausted plan was
claimed or recorded for that case.
