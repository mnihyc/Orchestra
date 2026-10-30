#!/usr/bin/env python3
"""First-thread binding on two connected machines with a real containerized harness."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

# This scenario's assigned ports keep it separate from every shared development stack.
API_PORT, GATEWAY_PORT, INTERNAL_PORT, MODEL_PORT, METRICS_PORT, PG_PORT, WEB_PORT = range(41360, 41367)
# A loaded development machine can need several minutes to start its services and harnesses.
WAIT_SECONDS = 180
# The optional browser check allows time to sign in and complete the start dialog.
BROWSER_SECONDS = 900
POLL_SECONDS = 1
MODEL = "mock-large"
KEY = "sk-orch-binding-scenario"
PREFIX = "orchbind-kfix"
IMAGE = "orch-dev-harness"
# A dedicated localhost name keeps browser cookies separate from other workers' loopback stacks.
WEB_HOST = "orchbind-kfix.localhost"


class Stack:
    def __init__(self, binaries, scratch, browser=False, frontend=None):
        self.repo = Path(__file__).resolve().parents[3]
        self.frontend = (frontend or self.repo / "frontend").resolve()
        self.binaries = binaries.resolve()
        self.scratch = scratch.resolve()
        self.scratch.mkdir(parents=True, exist_ok=True)
        self.containers = []
        self.children = []
        self.token = None
        self.evidence = []
        self.browser = browser
        self.machines = {}
        self.restarts = 0
        self.home = None

    def run(self, *args):
        return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()

    def container(self, name, image, *args, command=()):
        name = f"{PREFIX}-{name}"
        self.run("docker", "run", "-d", "--name", name, *args, image, *command)
        self.containers.append(name)
        return name

    def service(self, name, program, *args):
        log = (self.scratch / f"{name}.log").open("w")
        command = ["docker", "run", "--rm", "--pull=never", "--name", f"{PREFIX}-{name}", "--network", "host",
                   "-u", f"{os.getuid()}:{os.getgid()}",
                   "-v", f"{self.binaries}:/bin/orch:ro", "-v", f"{self.repo}:/repo:ro",
                   "-v", f"{self.scratch}:/data", "-e", f"ORCH_DATABASE_URL=postgres://orch:binding-test@127.0.0.1:{PG_PORT}/orchbind",
                   "--entrypoint", f"/bin/orch/{program}", IMAGE, *args]
        child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
        self.children.append((child, log))
        self.containers.append(f"{PREFIX}-{name}")

    def api(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        request = urllib.request.Request(f"http://127.0.0.1:{API_PORT}/api/v1{path}", data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=WAIT_SECONDS) as response:
                payload = response.read()
                return response.status, json.loads(payload) if payload else None
        except urllib.error.HTTPError as error:
            return error.code, json.loads(error.read())

    def call(self, method, path, body=None):
        status, reply = self.api(method, path, body)
        assert status < 300, (method, path, status, reply)
        return reply

    def wait(self, description, check, seconds=WAIT_SECONDS):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            try:
                value = check()
                if value:
                    return value
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(POLL_SECONDS)
        raise AssertionError(f"waiting for {description} timed out")

    def machine(self, label, roles, policy):
        print(f"Enrolling {label} with roles {roles}", flush=True)
        directory = self.scratch / label
        directory.mkdir(exist_ok=True)
        (directory / "config.toml").write_text("[connector]\nroles = " + json.dumps(roles) + "\n")
        name = self.container(label, IMAGE, "--pull=never", "--network", "host", "-u", f"{os.getuid()}:{os.getgid()}",
                              "-v", f"{self.binaries}:/bin/orch:ro", "-v", f"{directory}:/data",
                              "-v", f"{self.scratch / 'workspace'}:/work", "--entrypoint", "sleep", command=("infinity",))
        token = self.call("POST", "/machines/enrollment-tokens", {"name_hint": label})["token"]
        cli = ["docker", "exec", name, "/bin/orch/orch-connector", "--data-dir", "/data"]
        self.run(*cli, "enroll", "--url", f"https://127.0.0.1:{GATEWAY_PORT}", "--token", token, "--name", label)
        enrolled = next(m for m in self.call("GET", "/machines") if m["name"] == label)
        if "provider" in roles:
            path = f"/machines/{enrolled['id']}/providers"
            body = {"home_id": self.home, "kind": "api", "name": "Waiting for facts", "upstreams": [{"protocol": "openai-responses", "base_url": policy["provider.upstreams"][0], "options": {}}]}
        else:
            path = f"/machines/{enrolled['id']}/agents"
            body = {"home_id": self.home, "name": "Waiting for facts", "harness_kind": "codex"}
        status, pending = self.api("POST", path, body)
        assert status == 503 and pending["code"] == "source.offline" and pending["reason"] == "machine_facts_pending", (status, pending)
        self.evidence.append({"enrolled_before_initial_facts": enrolled, "readiness": pending})
        for key, value in policy.items():
            self.run(*cli, "policy", "set", key, json.dumps(value))
        log = (self.scratch / f"{label}.log").open("w")
        child = subprocess.Popen([*cli, "run"], stdout=log, stderr=subprocess.STDOUT)
        self.children.append((child, log))
        self.machines[label] = (name, cli)
        return self.wait(label + " facts", lambda: next((m for m in self.call("GET", "/machines") if m["name"] == label and "adapters" in m["facts"]), None))

    def policy(self, label, key, value):
        name, cli = self.machines[label]
        self.run(*cli, "policy", "set", key, json.dumps(value))
        self.run("docker", "restart", name)
        self.restarts += 1
        log = (self.scratch / f"{label}-restart-{self.restarts}.log").open("w")
        child = subprocess.Popen([*cli, "run"], stdout=log, stderr=subprocess.STDOUT)
        self.children.append((child, log))
        return self.wait("changed " + key, lambda: next((m for m in self.call("GET", "/machines") if m["name"] == label and m["facts"].get("policy", {}).get(key) == value), None))

    def start(self):
        print("Starting an empty binding stack", flush=True)
        for program in ["orch-api", "orch-gateway", "orch-connector", "orch-mock-llm", "orchctl"]:
            assert (self.binaries / program).is_file(), f"build {program} first"
        workspace = self.scratch / "workspace"
        workspace.mkdir(exist_ok=True)
        (workspace / "binding.txt").write_text("The harness works here; its model key stays on the provider machine.\n")
        config = f'''version = 1
[system]
name = "Binding scenario"
public_url = "http://{WEB_HOST}:{WEB_PORT}"
[api]
bind = "127.0.0.1:{API_PORT}"
internal_bind = "127.0.0.1:{INTERNAL_PORT}"
metrics_bind = "127.0.0.1:{METRICS_PORT}"
[gateway]
pinned_bind = "127.0.0.1:{GATEWAY_PORT}"
edge_bind = "127.0.0.1:41367"
metrics_bind = "127.0.0.1:41368"
backplane_url = "wss://127.0.0.1:{INTERNAL_PORT}/internal/backplane"
gateway_id = "orchbind"
managed_endpoint = "wss://127.0.0.1:{GATEWAY_PORT}"
[[gateway.endpoints]]
url = "wss://127.0.0.1:{GATEWAY_PORT}"
trust = "pinned"
[database]
migrations_dir = "/repo/migrations"
[storage]
root = "/data/store"
[pki]
service_dns_names = ["127.0.0.1", "api", "gateway"]
'''
        (self.scratch / "orch.toml").write_text(config)
        self.container("pg", "postgres:17-alpine@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24", "-p", f"127.0.0.1:{PG_PORT}:5432",
                       "--tmpfs", "/var/lib/postgresql/data", "-e", "POSTGRES_USER=orch", "-e", "POSTGRES_PASSWORD=binding-test", "-e", "POSTGRES_DB=orchbind")
        self.run("docker", "run", "--rm", "--pull=never", "--network", "host", "-u", f"{os.getuid()}:{os.getgid()}",
                 "-v", f"{self.binaries}:/bin/orch:ro", "-v", f"{self.scratch}:/data",
                 "--entrypoint", "/bin/orch/orchctl", IMAGE,
                 "compose-secrets", "--config", "/data/orch.toml", "--secrets-dir", "/data/secrets")
        self.service("api", "orch-api", "--config", "/data/orch.toml", "--secrets-dir", "/data/secrets", "--plain-http")
        self.wait("API", lambda: self.api("GET", "/meta")[0] == 200)
        self.service("gateway", "orch-gateway", "--config", "/data/orch.toml", "--secrets-dir", "/data/secrets")
        (self.scratch / "mock.key").write_text(KEY)
        self.service("model", "orch-mock-llm", "--listen", f"127.0.0.1:{MODEL_PORT}", "--credential-file", "/data/mock.key", "--model", MODEL)
        self.token = self.call("POST", "/setup", {"username": "binding-owner", "display_name": "Binding owner", "password": "binding scenario password"})["access_token"]
        org = self.call("POST", "/orgs", {"name": "Binding work", "slug": "binding-work"})
        project = self.call("POST", f"/orgs/{org['id']}/projects", {"name": "First work"})
        topic = self.call("POST", f"/projects/{project['id']}/topics", {"name": "First topic"})
        home, topic_id = org["id"], topic["id"]
        self.home = home
        self.call("PUT", f"/nodes/{home}/settings/providers.allow_http_upstreams", {"value": True})
        upstream = f"http://127.0.0.1:{MODEL_PORT}/v1"
        provider_machine = self.machine("provider", ["provider"], {"provider.upstreams": [upstream]})
        provider = self.call("POST", f"/machines/{provider_machine['id']}/providers", {"home_id": home, "kind": "api", "name": "Scripted endpoint", "upstreams": [{"protocol": "openai-responses", "base_url": upstream, "options": {}}]})
        self.call("PUT", f"/providers/{provider['id']}/key", {"api_key": KEY})
        self.call("POST", f"/providers/{provider['id']}/models", {"model_name": MODEL, "display_name": "Mock Large", "parameters": {"rate": {"input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15"}}})
        self.call("POST", f"/providers/{provider['id']}/refresh")
        environment_machine = self.machine("environment", ["agent", "environment"], {"environment.roots": ["/work"], "environment.remote_write": True, "agent.harnesses": [{"kind": "codex", "path": "/usr/local/bin/codex"}], "agent.max_sandbox": "open"})
        environment = self.call("POST", f"/machines/{environment_machine['id']}/environments", {"home_id": home, "home_node_id": topic_id, "name": "Main", "root": "/work", "remote_write": True})
        self.call("PUT", f"/topics/{topic_id}/default-environment", {"environment_id": environment["id"]})
        profile = self.call("POST", f"/machines/{environment_machine['id']}/agents", {"home_id": home, "name": "Codex beside workspace", "harness_kind": "codex"})
        agent = self.wait("actual Codex descriptor", lambda: next((a for a in self.call("GET", "/agents") if a["id"] == profile["id"] and a["descriptor"]), None))
        print("Actual Codex descriptor received before the first thread", flush=True)
        assert not self.call("GET", f"/nodes/{topic_id}/children?kind=thread")["items"]
        for key, value in {"agent.default_provider_id": provider["id"], "agent.default_agent_id": agent["id"], "agent.model": MODEL, "sandbox.mode": "open", "approval.mode": "auto"}.items():
            self.call("PUT", f"/nodes/{topic_id}/settings/{key}", {"value": value})
        options = self.call("GET", f"/nodes/{topic_id}/binding/options?environment_id={environment['id']}")
        offered = next(p for p in options["providers"] if p["provider"]["id"] == provider["id"])
        choices = next(a for a in offered["agents"] if a["agent"]["id"] == agent["id"])["arrangements"]
        first = min((a for a in choices if a["available"] and a["permitted"]), key=lambda a: a["rank"])
        assert first["type"] == "key_separated", choices
        separated = next(a for a in choices if a["type"] == "separated")
        assert provider_machine["roles"] == ["provider"], provider_machine
        assert not separated["available"] and separated["reason"] and separated["detail"], separated
        self.evidence.append({"options": options, "agent": agent, "environment": environment})
        body = {"name": "First binding", "binding": {"provider_id": provider["id"], "agent_id": agent["id"]}, "model": MODEL, "environment": {"use": environment["id"]}}
        body["binding"]["arrangement"] = "separated"
        status, refused = self.api("POST", f"/topics/{topic_id}/threads", body)
        assert status == 409 and refused["code"] == "binding.incompatible", (status, refused)
        assert refused["reason"] == separated["reason"], refused
        del body["binding"]["arrangement"]
        if self.browser:
            assert (self.frontend / "dist/index.html").is_file(), "build the browser application first"
            log = (self.scratch / "web.log").open("w")
            child = subprocess.Popen(["docker", "run", "--rm", "--pull=never", "--name", f"{PREFIX}-web", "--network", "host",
                "-u", f"{os.getuid()}:{os.getgid()}", "-v", f"{self.frontend}:/frontend:ro", "-w", "/frontend",
                "-e", f"ORCH_DEV_API=http://127.0.0.1:{API_PORT}", "--entrypoint", "npm", IMAGE,
                # Native loading avoids bundled-config scratch files in the read-only checkout.
                "run", "preview", "--", "--configLoader", "native", "--host", "127.0.0.1", "--port", str(WEB_PORT), "--strictPort"], stdout=log, stderr=subprocess.STDOUT)
            self.children.append((child, log))
            self.containers.append(f"{PREFIX}-web")
            (self.scratch / "browser.json").write_text(json.dumps({"topic": topic_id, "provider": provider["id"], "agent": agent["id"], "environment": environment["id"]}))
            print(f"Waiting for the browser's first thread at http://{WEB_HOST}:{WEB_PORT}", flush=True)
            thread = self.wait("browser-created first thread", lambda: self.first_thread(topic_id), BROWSER_SECONDS)
        else:
            thread = self.call("POST", f"/topics/{topic_id}/threads", body)
        assert thread["binding"]["arrangement"] == first["type"]
        assert thread["binding"]["isolation"]["harness_runs_in"] == "environment"
        assert thread["binding"]["isolation"]["held_by"] == "provider"
        if not self.browser:
            self.call("POST", f"/threads/{thread['id']}/inputs", {"content": [{"type": "text", "text": "Say ready."}], "delivery": "queue", "client_id": "first-turn"})
        sync = self.wait("first real harness turn", lambda: self.completed(thread["id"]))
        self.evidence.append({"thread": thread, "sync": sync})
        if self.browser:
            print("First browser turn completed; inspect the reply, then create browser.checked in the scratch directory", flush=True)
            self.wait("browser review", lambda: (self.scratch / "browser.checked").is_file(), BROWSER_SECONDS)
        plan = self.call("POST", f"/threads/{thread['id']}/forks/preflight", {"result": "new", "name": "Placement preview"})
        assert plan["to"]["isolation"]["harness_runs_in"] == "environment", plan
        assert plan["to"]["isolation"]["held_by"] == "provider", plan
        current = self.call("GET", f"/threads/{thread['id']}/binding/options")
        self.evidence.append({"fork": plan, "binding_options": current, "separated_refusal": refused})
        self.policy("provider", "provider.arrangements", ["separated"])
        denied = self.call("GET", f"/nodes/{topic_id}/binding/options?environment_id={environment['id']}")
        choice = self.choice(denied, provider["id"], agent["id"], "key_separated")
        assert choice["available"] and not choice["permitted"] and choice["reason"] == "arrangement_not_allowed", choice
        body["binding"]["arrangement"] = "key_separated"
        body["name"] = "Not delegated"
        status, delegation = self.api("POST", f"/topics/{topic_id}/threads", body)
        assert status == 409 and delegation["reason"] == "arrangement_not_allowed", (status, delegation)
        self.policy("provider", "provider.arrangements", ["separated", "key_separated"])
        self.policy("environment", "agent.harnesses", [])
        unavailable = self.wait("missing local harness", lambda: self.missing_harness(topic_id, environment["id"], provider["id"], agent["id"]))
        body["name"] = "No local harness"
        status, unsupported = self.api("POST", f"/topics/{topic_id}/threads", body)
        assert status == 409 and unsupported["reason"] == "agent_not_on_that_machine", (status, unsupported)
        self.evidence.append({"local_policy_denial": denied, "delegation_refusal": delegation, "missing_harness": unavailable, "unsupported_refusal": unsupported})
        (self.scratch / "evidence.json").write_text(json.dumps(self.evidence, indent=2))
        print("PASS first real harness turn; server default, binding, fork and credential holder agree", flush=True)

    def choice(self, options, provider, agent, kind):
        offered = next(p for p in options["providers"] if p["provider"]["id"] == provider)
        profile = next(a for a in offered["agents"] if a["agent"]["id"] == agent)
        return next(a for a in profile["arrangements"] if a["type"] == kind)

    def missing_harness(self, topic, environment, provider, agent):
        options = self.call("GET", f"/nodes/{topic}/binding/options?environment_id={environment}")
        choice = self.choice(options, provider, agent, "key_separated")
        return options if not choice["available"] and "harness" in choice["detail"].lower() else None

    def first_thread(self, topic):
        rows = self.call("GET", f"/nodes/{topic}/children?kind=thread")["items"]
        if not rows:
            return None
        return self.call("GET", f"/nodes/{rows[0]['id']}")

    def completed(self, thread):
        sync = self.call("GET", f"/threads/{thread}/sync?since=0")
        if not sync["turns"]:
            return None
        turn = sync["turns"][-1]
        assert turn["state"] not in ["failed", "cancelled"], turn
        return sync if turn["state"] == "completed" else None

    def close(self):
        for name in reversed(self.containers):
            subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
        for child, log in self.children:
            try:
                child.wait(timeout=WAIT_SECONDS)
            except subprocess.TimeoutExpired:
                child.terminate()
                child.wait(timeout=WAIT_SECONDS)
            log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True, help="static debug binaries built from this tree")
    parser.add_argument("--scratch", type=Path, required=True, help="an unused directory on the larger disk")
    parser.add_argument("--browser", action="store_true", help="serve the built web application and wait for its first thread and input")
    parser.add_argument("--frontend-dir", type=Path, help="a separately built browser checkout to check against this server")
    args = parser.parse_args()
    stack = Stack(args.bin_dir, args.scratch, args.browser, args.frontend_dir)
    try:
        stack.start()
    finally:
        stack.close()


if __name__ == "__main__":
    main()
