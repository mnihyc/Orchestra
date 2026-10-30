#!/usr/bin/env python3
"""Create a connected environment and real fake-harness binding by the public API."""
import argparse
import json
import os
from pathlib import Path
import time
import urllib.error
import urllib.request


class API:
    def __init__(self, edge, token):
        self.edge, self.token = edge, token

    def call(self, method, path, body=None):
        request = urllib.request.Request(self.edge + '/api/v1' + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'authorization': 'Bearer ' + self.token, 'content-type': 'application/json'})
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read(8 * 1024 * 1024 + 1)
            if len(raw) > 8 * 1024 * 1024:
                raise ValueError('the API answer exceeded its bound')
            return json.loads(raw) if raw else None


def until(description, read, predicate, seconds=180):
    deadline = time.monotonic() + seconds
    while True:
        value = read()
        if predicate(value):
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError(description)
        time.sleep(.25)  # Observation cadence, never proof that work ended.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('edge', 'system', 'token-file', 'model-key', 'model', 'out', 'ca'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    api = API(args.edge, Path(args.token_file).read_text())
    machine = until('the fixture connector did not publish its online policy', lambda: api.call('GET', '/machines'),
        lambda rows: any(row['name'] == 'release-proof' and row['online'] and
            (row.get('facts') or {}).get('policy', {}).get('provider.upstreams') for row in rows))
    machine = next(row for row in machine if row['name'] == 'release-proof' and row['online'])
    home = api.call('GET', '/me')['personal_org_id']
    project = api.call('POST', f'/orgs/{home}/projects', {'name': 'Release proof'})['id']
    topic = api.call('POST', f'/projects/{project}/topics', {'name': 'Measured work'})['id']
    api.call('PUT', f'/nodes/{home}/settings/providers.allow_http_upstreams', {'value': True})
    provider = api.call('POST', f'/machines/{machine["id"]}/providers', {'kind': 'api', 'home_id': home,
        'name': 'Scripted fixture', 'upstreams': [{'protocol': 'openai-responses', 'base_url': args.model, 'options': {}}]})['id']
    api.call('PUT', f'/providers/{provider}/key', {'api_key': Path(args.model_key).read_text().strip()})
    api.call('POST', f'/providers/{provider}/models', {'model_name': 'mock-large', 'display_name': 'Fixture',
        'parameters': {'rate': {'input_per_mtok': '0', 'cached_input_per_mtok': '0', 'output_per_mtok': '0'}}})
    agent = next(row['id'] for row in api.call('GET', '/agents') if row['harness_kind'] == 'fake')
    environment = api.call('POST', f'/machines/{machine["id"]}/environments', {
        'home_id': home, 'home_node_id': project, 'name': 'Release fixture', 'root': '/workspace'})['id']
    until('the environment did not attach', lambda: api.call('GET', f'/environments/{environment}'),
        lambda row: row['status'] == 'running')
    out = Path(args.out)
    out.write_text(json.dumps({'edge': args.edge, 'system': args.system, 'ca': args.ca,
        'token_file': args.token_file, 'home': home, 'project': project, 'topic': topic,
        'provider': provider, 'agent': agent, 'environment': environment, 'machine': machine['id']}))
    os.chmod(out, 0o600)


if __name__ == '__main__':
    main()
