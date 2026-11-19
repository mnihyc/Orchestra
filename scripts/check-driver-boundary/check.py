"""Reject implementation-name decisions and implementation-specific shared fields."""
from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
import re
import sys
import unittest


@dataclass(frozen=True)
class Token:
    text: str
    line: int
    literal: str | None = None


def tokens(source: str, javascript: bool = False) -> list[Token]:
    """Keep literal values and code tokens, discarding comments and whitespace."""
    single = r"(?:'(?:\\.|[^'\\\n])*'|`[A-Za-z][A-Za-z0-9_.-]*`)" if javascript else r"'(?:\\.|[^'\\\n])'"
    expression = re.compile(
        r'(?P<comment>//[^\n]*|/\*.*?\*/)|(?P<space>\s+)'
        r'|(?P<raw>\br(?P<hashes>\#+)".*?"(?P=hashes))'
        r'|(?P<string>"(?:\\.|[^"\\])*"|' + single + r')'
        r'|(?P<word>[A-Za-z_$][A-Za-z0-9_$]*)'
        r'|(?P<operator>===|!==|==|!=|=>|&&|\|\||::|.)',
        re.DOTALL,
    )
    result = []
    line = 1
    for match in expression.finditer(source):
        token_line = line
        line += match.group().count('\n')
        if match.lastgroup in ('comment', 'space'):
            continue
        value = match.group()
        literal = None
        if match.lastgroup == 'string':
            literal = value[1:-1]
            try:
                if '\\' in literal:
                    literal = json.loads(value if value.startswith('"') else '"' + literal.replace('"', '\\"') + '"')
            except (ValueError, SyntaxError):
                literal = value[1:-1]
        elif match.lastgroup == 'raw':
            hashes = match.group('hashes')
            literal = value[len(hashes) + 2: -len(hashes) - 1]
        result.append(Token(value, token_line, literal))
    return result


def branch_faults(source: str, names: set[str], javascript: bool = False) -> list[int]:
    stream = tokens(source, javascript)
    faults = set()
    driver_words = {'driver', 'driver_name', 'driverName', 'selectedDriver'}
    def driver_identity(parts: list[Token]) -> bool:
        expression = ''.join(part.text for part in parts if part.text not in ('if', 'return'))
        expression = re.sub(r'\.(?:as_str|as_ref|as_deref)\(\)', '', expression).strip('()')
        if expression.endswith('.name') and 'driver' in expression.lower():
            return True
        tail = re.search(r'([A-Za-z_$][A-Za-z0-9_$]*)$', expression)
        return tail is not None and re.fullmatch(r'.*driver(?:_?name)?', tail.group().lower()) is not None

    def named_literal(parts: list[Token]) -> bool:
        return any(part.literal is not None and re.fullmatch(r'[A-Za-z][A-Za-z0-9_.-]*', part.literal) for part in parts)

    def selects_driver(condition: list[Token]) -> bool:
        literal_names = {part.literal for part in condition} & names
        if not literal_names:
            return False
        # Host and directory are also ordinary object kinds and field keys.
        # A generic `name` is sufficient only for names without those other meanings.
        return any(part.text in driver_words for part in condition) or (
            bool(literal_names - {'host', 'directory'}) and any(part.text == 'name' for part in condition)
        )
    barriers = {';', '{', '}', ',', '&&', '||', '=>', '?', ':', '=', 'return'}
    for index, token in enumerate(stream):
        if token.text in ('==', '===', '!=', '!=='):
            start, end = index - 1, index + 1
            while start >= 0 and stream[start].text not in barriers:
                start -= 1
            while end < len(stream) and stream[end].text not in barriers:
                end += 1
            condition = stream[start + 1:end]
            left, right = stream[start + 1:index], stream[index + 1:end]
            if selects_driver(condition) or (driver_identity(left) and named_literal(right)) or (driver_identity(right) and named_literal(left)):
                faults.add(token.line)
        if token.text == 'if' or (token.text == 'matches' and index + 1 < len(stream) and stream[index + 1].text == '!'):
            end = index + 1
            while end < len(stream) and stream[end].text not in ('{', ';', '=>'):
                end += 1
            condition = stream[index + 1:end]
            if selects_driver(condition):
                faults.add(token.line)
        if token.text in ('match', 'switch'):
            opening = index + 1
            while opening < len(stream) and stream[opening].text not in ('{', ';'):
                opening += 1
            if opening == len(stream) or stream[opening].text != '{':
                continue
            subject = stream[index + 1:opening]
            identity = driver_identity(subject)
            if not identity and not any(part.text in driver_words or part.text == 'name' for part in subject):
                continue
            depth, pattern = 1, True
            for part in stream[opening + 1:]:
                if part.text == '{':
                    depth += 1
                elif part.text == '}':
                    depth -= 1
                    if depth == 0:
                        break
                elif depth == 1 and part.text in (',', 'case'):
                    pattern = True
                elif depth == 1 and part.text in ('=>', ':'):
                    pattern = False
                elif depth == 1 and pattern and (part.literal in names or (identity and named_literal([part]))):
                    faults.add(part.line)
    return sorted(faults)


def shared_field_faults(source: str, path: str, serialized_only: bool = False) -> list[int]:
    stream = tokens(source)
    forbidden = re.compile(r'(?:docker[A-Za-z0-9_]*|container_?(?:id|name)|daemon_?id)\Z', re.IGNORECASE)
    faults = []
    pending_type = None
    types: list[tuple[str, int, str, bool, bool]] = []
    depth = 0
    square_depth = 0
    for index, token in enumerate(stream):
        if token.text == "[":
            square_depth += 1
        elif token.text == "]":
            square_depth -= 1
        if token.text in ('struct', 'enum') and index + 1 < len(stream):
            start = index - 1
            while start >= 0 and stream[start].text not in (';', '{', '}'):
                start -= 1
            serialized = any(part.text in ('Serialize', 'Deserialize') for part in stream[start + 1:index])
            error_type = any(part.text in ('Error', 'thiserror') for part in stream[start + 1:index])
            check_enum = serialized or (not serialized_only and not error_type)
            pending_type = (stream[index + 1].text, token.text, not serialized_only or serialized, check_enum)
        elif token.text == ';':
            pending_type = None
        elif token.text == '{':
            depth += 1
            if pending_type is not None:
                types.append((pending_type[0], depth, pending_type[1], pending_type[2], pending_type[3]))
                pending_type = None
        elif token.text == '}':
            if types and types[-1][1] == depth:
                types.pop()
            depth -= 1
        if types and types[-1][3] and depth <= types[-1][1] + 1 and index + 1 < len(stream) and stream[index + 1].text == ':' and forbidden.fullmatch(token.text):
            # A source format already on the wire is not an implementation selector.
            existing_format = (path, types[-1][0], token.text) == ('crates/contracts/driver/src/sources.rs', 'BuildSource', 'dockerfile')
            if not existing_format:
                faults.append(token.line)
        # Renamed fields and enum values count too; aliases are input compatibility only.
        if types and types[-1][4] and square_depth == 0 and types[-1][2] == 'enum' and depth == types[-1][1] and forbidden.fullmatch(token.text) and index + 1 < len(stream) and stream[index + 1].text in (',', '{', '(', '=', '}'):
            faults.append(token.line)
        if types and types[-1][3] and token.literal and forbidden.fullmatch(token.literal) and index >= 2 and stream[index - 2].text == 'rename':
            faults.append(token.line)
    return sorted(set(faults))


def check(root: Path) -> list[str]:
    root = root.resolve()
    names = {directory.name for directory in (root / 'extensions/drivers').iterdir() if directory.is_dir()}
    names.add('directory')
    faults = []
    for directory in ('crates', 'apps', 'frontend/src'):
        for path in sorted((root / directory).rglob('*')):
            relative_path = path.relative_to(root)
            if path.suffix not in ('.rs', '.ts', '.tsx') or 'tests' in relative_path.parts or 'generated' in relative_path.parts:
                continue
            relative = relative_path.as_posix()
            source = path.read_text()
            if path.suffix == '.rs':
                source = source.split('#[cfg(test)]', 1)[0]
            for line in branch_faults(source, names, path.suffix != '.rs'):
                faults.append(f'{relative}:{line}: behavior selects an implementation name')
            if path.suffix == '.rs':
                shared = relative.startswith(('crates/contracts/', 'crates/control/db/'))
                for line in shared_field_faults(source, relative, serialized_only=not shared):
                    faults.append(f'{relative}:{line}: shared data names an implementation object')
    for path in sorted((root / 'migrations').glob('*.sql')):
        source = re.sub(r'--[^\n]*|/\*.*?\*/|\'(?:\'\'|[^\'])*\'', '', path.read_text(), flags=re.DOTALL)
        if re.search(r'\b(?:docker[A-Za-z0-9_]*|container_id|container_name|daemon_id)\b', source, flags=re.IGNORECASE):
            faults.append(f'{path.relative_to(root)}: stored columns name an implementation object')
    return faults


class GuardCases(unittest.TestCase):
    def test_name_decisions_in_each_supported_syntax_fail(self):
        names = {'docker', 'host', 'directory'}
        for source in ('if driver == "docker" { run(); }', 'if "docker" != spec.driver { run(); }', 'match driver.as_str() { "host" => a(), _ => b() }', 'matches!(driver.name.as_str(), "docker" | "host")', "const chosen = driver === 'docker' ? one : two", 'if (driver === `docker`) { run(); }', "const named = `${driver === 'docker' ? 1 : 0}`", "switch (driver) { case 'host': one(); }", 'if selectedDriverName == "cloud-sandbox" { run(); }', 'match driver.as_str() { "cloud-sandbox" => a(), _ => b() }'):
            self.assertTrue(branch_faults(source, names, True), source)

    def test_registrations_identity_routing_and_resource_kinds_remain_generic(self):
        for source in ('register("docker", factory);', 'if spec.driver == descriptor.name { run(); }', 'if driver.process_scope == "all" { show(); }', 'if object.kind == "host" { show(); }', 'if origin["name"] != body.name || origin["directory"] != body.directory { refuse(); }', '// if driver == "docker" {}\nlet name = "memory";'):
            self.assertEqual(branch_faults(source, {'docker', 'host'}), [], source)

    def test_new_wire_or_stored_implementation_members_fail(self):
        for source in ('pub struct Request { pub docker_id: String }', 'pub struct Row { pub container_id: String }', 'pub struct Row { pub containerId: String }', 'pub struct Request { pub DockerId: String }', 'pub struct Request { #[serde(rename="docker_id")] pub native: String }', 'pub enum Resource { Unit { container_id: String } }', 'pub struct Request { pub dockerfile: String }'):
            self.assertTrue(shared_field_faults(source, 'crates/contracts/example.rs'), source)
        self.assertEqual(shared_field_faults('pub struct Request { pub reference: String, pub driver: String }', 'contract.rs'), [])
        self.assertEqual(shared_field_faults('pub struct BuildSource { pub dockerfile: String }', 'crates/contracts/driver/src/sources.rs'), [])
        self.assertEqual(shared_field_faults('fn build() { let source = BuildSource { dockerfile: String::new() }; }', 'factory.rs'), [])
        self.assertTrue(shared_field_faults('#[derive(Serialize, Deserialize)] struct Record { docker_id: String }', 'crates/runtime/node/src/ledger.rs', serialized_only=True))
        self.assertTrue(shared_field_faults('#[derive(Serialize)] enum Resource { DockerVolume { reference: String } }', 'state.rs', serialized_only=True))
        self.assertTrue(shared_field_faults('pub enum Resource { DockerVolume { reference: String } }', 'crates/control/db/src/row.rs'))
        self.assertEqual(shared_field_faults('struct DockerExtras { docker: Docker }', 'composition.rs', serialized_only=True), [])
        self.assertEqual(shared_field_faults('use serde::{Serialize, Deserialize}; struct DockerExtras { docker: Docker }', 'composition.rs', serialized_only=True), [])
        self.assertEqual(shared_field_faults('#[derive(Debug, thiserror::Error)] enum SourceError { #[error("too long {}", DOCKERFILE_MAX_BYTES / 1024)] DockerfileTooLong }', 'sources.rs'), [])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(GuardCases)
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    if not result.wasSuccessful():
        return 1
    faults = check(args.root)
    if faults:
        print('\n'.join(faults))
        return 1
    print('clean driver boundary')
    return 0


if __name__ == '__main__':
    sys.exit(main())
