"""Nonsecret endpoint descriptions; model requests and their sign-in stay in the native harness.

This is the examples' x-endpoint.describe extension, not credential.present and not an HTTP server.
Endpoint options follow the same protocol authorize defaults as the connector's plain forwarder.
"""

from plan import Refusal

# Codex's pinned model-provider-info CHATGPT_CODEX_BASE_URL; Claude Code's native Messages origin.
# No account credential, bearer token or environment value is read by this module.
NATIVE = {
    "codex": {"protocol": "openai-responses", "base_url": "https://chatgpt.com/backend-api/codex",
              "path": "/responses", "headers": {}, "body": {"store": False, "stream": True},
              "dynamic_headers": ["chatgpt-account-id", "originator", "user-agent"]},
    "claude": {"protocol": "anthropic-messages", "base_url": "https://api.anthropic.com",
               "path": "/v1/messages", "headers": {"anthropic-version": "2023-06-01", "anthropic-beta": "oauth-2025-04-20"},
               "body": {"stream": True}, "dynamic_headers": ["x-app", "user-agent"]},
}


def endpoint(kind, params, models):
    native = NATIVE[kind]
    configured = [entry for entry in params.get("endpoints", []) if entry.get("protocol") == native["protocol"]]
    if configured and any(entry.get("base_url") != native["base_url"] for entry in configured):
        raise Refusal("refused", "the example describes its existing native endpoint, not a credential-bearing proxy")
    return {"endpoint": {"protocol": native["protocol"], "base_url": native["base_url"],
                         "options": {"auth": "bearer", "headers": native["headers"]}},
            "presents": {"header": "authorization", "scheme": "Bearer", "by": "native harness",
                         "credential": "the same native sign-in; never returned by this program"},
            "request": {"method": "POST", "path": native["path"], "body": native["body"],
                        "dynamic_headers": native["dynamic_headers"], "preserve_harness_headers": True},
            **models}
