#!/usr/bin/env python3
"""Inspect immutable image indexes and check executable build inputs offline."""

import argparse
import ast
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys

# Registry outages must not hold a release review indefinitely.
REGISTRY_SECONDS = 90
INDEX_TYPES = {"application/vnd.oci.image.index.v1+json", "application/vnd.docker.distribution.manifest.list.v2+json"}
PIN = re.compile(r"[^@\s]+:[^:@/\s]+@sha256:[0-9a-f]{64}\Z")
# Executable inputs are discovered throughout the tree, including test network
# recipes. Documentation strings and test assertions do not select build inputs.
CODE_SUFFIXES = {".py", ".sh", ".yml", ".yaml", ".json", ".toml"}
# Docker's flag-only options are distinct from the options taking the next
# argument; that distinction prevents mistaking a label or mount for an image.
RUN_FLAGS = {"--rm", "--init", "--read-only", "--privileged", "--detach", "--interactive", "--tty", "-d", "-i", "-t", "-it", "-dit", "--help", "--quiet", "-q", "--no-healthcheck", "--publish-all", "-P", "--oom-kill-disable"}
VALUE_OPTIONS = {"--add-host", "--attach", "-a", "--cap-add", "--cap-drop", "--cgroupns", "--cidfile", "--cpus", "--cpu-shares", "--cpuset-cpus", "--device", "--dns", "--entrypoint", "--env", "-e", "--env-file", "--expose", "--gpus", "--group-add", "--health-cmd", "--health-interval", "--health-retries", "--health-start-period", "--health-timeout", "--hostname", "-h", "--ip", "--ip6", "--ipc", "--isolation", "--label", "-l", "--label-file", "--link", "--log-driver", "--log-opt", "--memory", "-m", "--memory-reservation", "--memory-swap", "--mount", "--name", "--network", "--network-alias", "--pid", "--pids-limit", "--platform", "--publish", "-p", "--pull", "--restart", "--runtime", "--security-opt", "--shm-size", "--stop-signal", "--stop-timeout", "--storage-opt", "--sysctl", "--tmpfs", "--ulimit", "--user", "-u", "--userns", "--uts", "--volume", "-v", "--volume-driver", "--volumes-from", "--workdir", "-w"}


def image_operand(tokens):
    if len(tokens) < 2 or tokens[0] != "docker" or tokens[1] not in {"run", "create", "pull"}: return None
    index = 2
    while index < len(tokens):
        token = tokens[index]
        if token is None:  # A list of runtime options expanded by the caller.
            index += 1
        elif token == "--":
            return index + 1 if index + 1 < len(tokens) else None
        elif token.startswith("-"):
            if token in RUN_FLAGS or "=" in token or tokens[1] == 'pull' and token in {'-a','--all-tags'}: index += 1
            elif token in VALUE_OPTIONS: index += 2
            else: raise ValueError(f'cannot determine image operand after unsupported Docker option {token}')
        else:
            return index
    return None


def python_images(text):
    tree = ast.parse(text)
    assignments = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign):
            for name in node.targets:
                if isinstance(name, ast.Name): assignments[name.id] = node.value
    def literal(node):
        if isinstance(node, ast.Constant) and isinstance(node.value, str): return node.value
        if isinstance(node, ast.JoinedStr): return "".join(part.value if isinstance(part, ast.Constant) else "{expression}" for part in node.values)
        return None
    defaults = set()
    selected = set()
    local_only = set()
    def keep(node, destination, local=False):
        if isinstance(node, ast.Name): node = assignments.get(node.id)
        if node is not None:
            value = literal(node)
            if value is not None:
                # Ignore absolute mounted paths and URI values of helper args.
                if not value.startswith(("/", "http://", "https://", "ws://", "wss://", "postgres://", "postgresql://")):
                    raw = ast.get_source_segment(text, node)
                    quote = re.search("[\"']", raw)
                    if quote:
                        reference=raw[quote.end():-1]
                        if local and ':' not in value and '/' not in value: local_only.add(reference)
                        else: destination.add(reference)
    for name, node in assignments.items():
        if name == "IMAGE" or name.endswith("_IMAGE"): keep(node,defaults)
    for node in ast.walk(tree):
        elements=node.elts if isinstance(node,(ast.List,ast.Tuple)) else node.args if isinstance(node,ast.Call) else []
        if elements:
            values=[literal(element) if literal(element) is not None else '$runtime' if isinstance(element, (ast.Name, ast.Attribute, ast.Call)) else None for element in elements]
            index=image_operand(values)
            if index is not None: keep(elements[index],selected,'--pull=never' in values)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "container" and len(node.args)>1:
            keep(node.args[1],selected,any(literal(arg)=='--pull=never' for arg in node.args))
    return (defaults-local_only)|selected


def shell_images(text):
    found=set()
    local_only=set()
    selectors=set()
    assignments={match[1]:match[2].strip("\"'") for match in re.finditer(r'''(?m)^\s*(?:export\s+)?([A-Z_][A-Z0-9_]*)=["']?([^\s"']+)''',text)}
    for name, reference in assignments.items():
        if name == 'IMAGE' or name.endswith('_IMAGE'):
            if not reference.startswith('$'): found.add(reference)
    for line in text.replace('\\\n',' ').splitlines():
        if line.lstrip().startswith('#'): continue
        match=re.search(r'\bdocker\s+(?:run|create|pull)(?=\s|$)',line)
        if not match: continue
        lexer=shlex.shlex(line[match.start():],posix=True)
        lexer.whitespace_split=True
        tokens=[]
        index=None
        for token in lexer:
            tokens.append(token)
            index=image_operand(tokens)
            if index is not None: break
        if index is not None:
            reference=tokens[index]
            if reference.startswith('$'): reference=assignments.get(reference.removeprefix('$').strip('{}'),'')
            if reference and not reference.startswith('$'):
                if '--pull=never' in tokens and ':' not in reference and '/' not in reference:
                    local_only.add(reference)
                else: selectors.add(reference)
    return (found-local_only)|selectors


def paths(root):
    # git's inventory also works for a worktree and includes newly added inputs.
    result = subprocess.run(["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z"], capture_output=True, check=False)
    if result.returncode == 0:
        return sorted({Path(path.decode()) for path in result.stdout.split(b"\0") if path})
    # A clean exported source archive has no git directory.
    return sorted(path.relative_to(root) for path in root.rglob("*") if path.is_file() and not any(part in {"node_modules", "target", ".git", ".tmp", "dist"} for part in path.relative_to(root).parts))


def inputs(root):
    compose = root / "deploy/compose.yml"
    database_recipe = root / "deploy/postgres/Dockerfile"
    if compose.exists() and 'build: ./postgres' in compose.read_text() and not database_recipe.exists():
        raise ValueError("deploy/postgres/Dockerfile: the database build recipe is missing")
    selected = []
    faults = []
    for relative in paths(root):
        path = root / relative
        recipe = "dockerfile" in path.name.lower()
        yaml = path.suffix in {".yml", ".yaml"}
        if not recipe and path.suffix not in CODE_SUFFIXES:
            if not path.read_bytes().startswith(b'#!'): continue
        if path.name.startswith("test_") or relative.parts[:3] == ('frontend','src','i18n'):
            continue
        text = path.read_text()
        matches = []
        if recipe:
            arguments = {match[1]: match[2].strip("\"'") for match in re.finditer(r"(?im)^\s*ARG\s+([A-Z_][A-Z0-9_]*)=([^\s#]+)", text)}
            stages = set()
            for match in re.finditer(r"(?im)^\s*FROM\s+(?:--platform=\S+\s+)?([^\s#]+)", text):
                reference = match[1]
                known_stage = reference in stages
                tail = text[match.end():].split('\n',1)[0]
                alias = re.search(r'(?i)\s+AS\s+(\S+)',tail)
                if alias: stages.add(alias[1])
                if reference.lower() == "scratch" or known_stage:
                    continue
                variable = re.fullmatch(r"\$\{?([A-Z_][A-Z0-9_]*)\}?", reference)
                if variable:
                    name = variable[1]
                    if name not in arguments:
                        faults.append(f"{relative}: {reference} has no immutable default")
                        continue
                    pattern = rf"(?im)^\s*ARG\s+{re.escape(name)}=([^\s#]+)"
                    matches.extend(re.finditer(pattern, text))
                else:
                    matches.append(match)
            matches.extend(re.finditer(r"(?im)^\s*ARG\s+[A-Z_]*IMAGE=([^\s#]+)",text))
        elif yaml:
            matches.extend(re.finditer(r"(?m)^[ \t]*(?:-[ \t]*)?(?:image|container):[ \t]*[\"']?([^\s\"'#{}]+)", text))
        # Script selectors are parsed at Docker calls and image defaults, rather
        # than searching arbitrary strings such as port mappings or URI schemes.
        if not recipe:
            try:
                references=python_images(text) if path.suffix=='.py' else shell_images(text) if path.suffix=='.sh' or text.startswith('#!/bin/bash') or text.startswith('#!/usr/bin/env bash') else set()
            except ValueError as error:
                raise ValueError(f'{relative}: {error}') from error
            for reference in references:
                matches.extend((match.start(),match.end(),reference) for match in re.finditer(re.escape(reference),text))
            matches.extend(match for match in re.finditer(r'''\b[A-Z_]*IMAGE=["']?([a-z0-9][^\s"'$]+)''',text) if ':' in match[1] or '/' in match[1])
            if path.suffix in {'.json','.toml'}:
                matches.extend(re.finditer(r'''(?m)(?:["']image["']\s*:|\bimage\s*=)\s*["']([^\s"']+)''',text))
        for match in matches:
            start, end, reference = match if isinstance(match, tuple) else (match.start(1), match.end(1), match[1].strip("\"'"))
            if reference.startswith(("$", "${{")):
                faults.append(f"{relative}: image selector {reference} needs a pinned default")
            else:
                selected.append((relative, start, end, reference))
    if faults:
        raise ValueError("\n".join(faults))
    return sorted(set(selected))


def check(root, selected):
    faults = [f"{path}: {reference} needs a tag and a sha256 index digest"
              for path, _, _, reference in selected if not PIN.fullmatch(reference)]
    for relative in paths(root):
        if relative.suffix not in {".yml", ".yaml"} or not (str(relative).startswith(".github/") or relative.name in {"action.yml", "action.yaml"}): continue
        for action in re.findall(r"^\s*(?:-\s*)?uses:\s*(\S+)", (root / relative).read_text(), re.MULTILINE):
            action = action.strip("\"'")
            if action.startswith("./"): continue
            if action.startswith("docker://"):
                if not PIN.fullmatch(action.removeprefix("docker://")): faults.append(f"{relative}: {action} needs a tag and a sha256 index digest")
            elif not re.fullmatch(r"[^@]+@[0-9a-f]{40}", action):
                faults.append(f"{relative}: {action} needs a full Action commit")
    for fault in faults: print(fault, file=sys.stderr)
    if not faults: print(f"{len(selected)} image selections and all Actions are pinned; checked offline")
    return int(bool(faults))


def inspect(tag):
    answer = subprocess.run(["docker", "buildx", "imagetools", "inspect", tag, "--format", "{{json .Manifest}}"], check=True, capture_output=True, text=True, timeout=REGISTRY_SECONDS)
    index = json.loads(answer.stdout)
    digest = index.get("digest", "")
    if index.get("mediaType") not in INDEX_TYPES or not index.get("manifests"):
        raise ValueError(f"{tag}: the registry did not return an image index")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError(f"{tag}: the registry did not return a sha256 index digest")
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="check all executable image inputs and Actions offline")
    mode.add_argument("--write", action="store_true", help="rewrite pins only after all registry lookups succeed")
    args = parser.parse_args()
    try:
        selected = inputs(args.root)
        if args.check: return check(args.root, selected)
        tags = sorted({reference.split("@", 1)[0] for _, _, _, reference in selected})
        if any(not re.fullmatch(r"[^@\s]+:[^:@/\s]+", tag) for tag in tags): raise ValueError("cannot refresh an unresolved or untagged image input")
        digests = {tag: inspect(tag) for tag in tags}
        changes = {}
        for relative, start, end, reference in selected:
            current = f"{reference.split('@', 1)[0]}@{digests[reference.split('@', 1)[0]]}"
            print(f"{relative}: {reference} -> {current}")
            changes.setdefault(relative, []).append((start, end, current))
        if args.write:
            for relative, replacements in changes.items():
                path = args.root / relative
                text = path.read_text()
                for start, end, current in sorted(set(replacements), reverse=True): text = text[:start] + current + text[end:]
                path.write_text(text)
            print("wrote the reviewed index pins")
        else: print("dry run; no files changed (use --write to refresh the pins)")
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"stock-images: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__": sys.exit(main())
