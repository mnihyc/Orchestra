"""Record functional SDK declaration names without installing or executing its package.

This is a source-evidence extractor, not a TypeScript checker or a harness parser.
The replay suite consumes the original native bytes through the actual adapter.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile


def declarations(text):
    text = re.sub(r"/\*.*?\*/", "", text.replace("\r\n", "\n"), flags=re.S)
    start = re.compile(r"(?:export\s+)?declare\s+(?:type|interface)\s+([A-Za-z_$][\w$]*)(?:\s*=)?\s*\{")
    result = {}
    for match in start.finditer(text):
        name = match.group(1)
        if name not in {
            "Settings", "ControlResponse", "ControlErrorResponse", "SDKAssistantMessage",
            "SDKUserMessage", "SDKResultSuccess", "SDKSystemMessage", "SDKPartialAssistantMessage",
            "SDKControlRequest", "SDKControlResponse", "SDKControlGetSettingsRequest",
            "SDKHookStartedMessage", "SDKHookResponseMessage", "SDKAPIRetryMessage",
            "SDKTaskNotificationMessage",
        }:
            continue
        first = at = match.end()
        depth, quote, escaped = 1, None, False
        while at < len(text) and depth:
            char = text[at]
            if quote:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quote:
                    quote = None
            elif char in "'\"`":
                quote = char
            elif char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
            at += 1
        if depth:
            raise ValueError(f"unterminated declaration {name}")
        body = text[first:at - 1]
        fields = {}
        for field in re.finditer(r"^    ([A-Za-z_$][\w$]*)(\?)?:\s*([^\n]*)", body, re.M):
            key, optional, value = field.groups()
            if key in fields:
                raise ValueError(f"ambiguous property {name}.{key}")
            value = value.strip().removesuffix(";")
            literals = re.findall(r"'([^']*)'", value)
            kind = ("boolean" if value == "boolean" else "number" if value == "number"
                    else "string" if value == "string" or literals else "object" if value.startswith("{")
                    else "reference")
            fields[key] = {"optional": bool(optional), "literals": literals, "kind": kind}
        result[name] = {
            "fields": fields,
            "property_names": sorted(set(re.findall(r"^\s+([A-Za-z_$][\w$]*)\??:", body, re.M))),
        }
    if "Settings" not in result or "SDKControlRequest" not in result:
        raise ValueError("the SDK no longer publishes the expected declaration entrypoints")
    return result


def extract(package, native_version, sdk_version):
    archive = package.read_bytes()
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
        package_meta = json.load(tar.extractfile("package/package.json"))
        if package_meta["name"] != "@anthropic-ai/claude-agent-sdk" or package_meta["version"] != sdk_version:
            raise ValueError("the supplied archive is not the named SDK build")
        raw = tar.extractfile("package/sdk.d.ts").read()
        scripts = {entry.name: tar.extractfile(entry).read() for entry in tar.getmembers()
                   if entry.isfile() and entry.name.endswith((".js", ".mjs"))}
    adapter = Path(__file__).resolve().parents[3]
    sessions = (adapter / "src/sessions.rs").read_text()
    process = (adapter / "src/process.rs").read_text()
    managed = sessions[sessions.index("pub(crate) fn managed_environment("):sessions.index("pub(crate) fn place_home(")]
    quiet = process[process.index("pub const QUIET_SWITCHES:"):process.index("const INHERITED:")]
    names = sorted(set(re.findall(r'"([A-Z][A-Z0-9_]+)"', managed + quiet)) | {
        "CLAUDE_CODE_PROMPT_CACHE_TTL", "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL",
        "CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK",
    })
    symbols = {name: any(name.encode() in source for source in scripts.values()) for name in names}
    if not all(symbols.values()):
        raise ValueError("managed environment names absent from the SDK: " + ", ".join(name for name in names if not symbols[name]))
    types = declarations(raw.decode())
    managed_settings = sessions[sessions.index("pub(crate) fn managed_settings("):sessions.index("pub(crate) fn expected_managed_pins(")]
    runtime_settings = {}
    for name in re.findall(r'settings\.insert\(\s*"([^"]+)"', managed_settings):
        if name in types["Settings"]["fields"]:
            continue
        pattern = re.compile(re.escape(name) + r"\s*:\s*\(\s*\)\s*=>\s*[A-Za-z_$][\w$]*\.boolean\(\)")
        for path, source in scripts.items():
            text = source.decode()
            match = pattern.search(text)
            if match:
                runtime_settings[name] = {"kind": "boolean", "optional": text[match.end():].startswith(".optional()"),
                                          "literals": [], "source": {"path": path, "sha256": hashlib.sha256(source).hexdigest()}}
                break
        if name not in runtime_settings:
            raise ValueError(f"managed setting {name} has neither a published type nor a recognized runtime schema")
    return {
        "format": 1, "program": "claude-code", "native_version": native_version,
        "evidence": {
            "kind": "published_protocol",
            "package": {"name": package_meta["name"], "version": sdk_version, "sha256": hashlib.sha256(archive).hexdigest()},
            "references": [f"https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-{sdk_version}.tgz",
                           "https://code.claude.com/docs/en/agent-sdk/typescript"],
            "declarations_file": {"path": "package/sdk.d.ts", "sha256": hashlib.sha256(raw).hexdigest()},
            "runtime_files": [{"path": path, "sha256": hashlib.sha256(source).hexdigest()} for path, source in scripts.items()],
        },
        "declarations": types, "runtime_setting_fields": runtime_settings, "managed_environment_symbols": symbols,
        "qualification_environment_symbols": {
            "CLAUDE_CODE_DISABLE_FEATURE_FLAGS": any(b"CLAUDE_CODE_DISABLE_FEATURE_FLAGS" in source for source in scripts.values()),
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--native-version", required=True)
    parser.add_argument("--sdk-version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    evidence = extract(args.package, args.native_version, args.sdk_version)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
