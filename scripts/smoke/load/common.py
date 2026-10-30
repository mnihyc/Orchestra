#!/usr/bin/env python3
"""Public API and exact-owned-container measurements shared by release load and soak."""
import json
import math
from pathlib import Path
import subprocess
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'link-load'))
from world import API, until


class Session(API):
    def call(self, method, path, body=None):
        try:
            return super().call(method, path, body)
        except urllib.error.HTTPError as error:
            if error.code != 401:
                raise
        # This is the disposable fixture administrator, not a person's saved credential.
        request = urllib.request.Request(self.edge + '/api/v1/auth/login', method='POST',
            data=json.dumps({'login': 'ada', 'password': 'correct horse battery'}).encode(),
            headers={'content-type': 'application/json'})
        with urllib.request.urlopen(request, timeout=60) as response:
            self.token = json.load(response)['access_token']
        return super().call(method, path, body)


def load_world(path):
    world = json.loads(path.read_text())
    return world, Session(world['edge'], Path(world['token_file']).read_text())


def thread(api, world, name):
    return api.call('POST', f'/topics/{world["topic"]}/threads', {'name': name,
        'binding': {'agent_id': world['agent'], 'provider_id': world['provider']},
        'model': 'mock-large', 'environment': {'use': world['environment']}, 'directory': '.'})['id']


def input_body(key, marker, delay_ms=0):
    return {'content': [{'type': 'text', 'text': f'#!fake\nsleep {delay_ms}\nusage 10 2\nsay {marker}'}],
        'delivery': 'queue', 'client_id': key}


def settled(api, thread_id, input_id, guard):
    return until('an accepted input has no explicit terminal outcome',
        lambda: api.call('GET', f'/threads/{thread_id}/sync'),
        lambda row: any(turn['input_id'] == input_id and turn['state'] in ('completed', 'failed', 'interrupted')
            for turn in row['turns']), seconds=guard)


def check_work(sync, input_id, marker, allow_interruption=False):
    inputs = [row for row in sync['inputs'] if row['id'] == input_id]
    turns = [row for row in sync['turns'] if row['input_id'] == input_id]
    messages = [row for row in sync['items'] if row['kind'] == 'agent_message' and row['payload'].get('text') == marker]
    if len(inputs) != 1 or len(turns) != 1 or len(messages) > 1:
        raise ValueError('an accepted input, turn or output was lost or duplicated')
    state = turns[0]['state']
    if state == 'completed' and len(messages) != 1:
        raise ValueError('a completed turn lost its exact output')
    if state != 'completed' and not (allow_interruption and state in ('failed', 'interrupted')):
        raise ValueError('the expected turn did not complete: ' + state)
    return {'input': input_id, 'turn': turns[0]['id'], 'state': state, 'outputs': len(messages),
        'seq': sync['seq']}


def percentile(values, fraction):
    if not values:
        raise ValueError('there are no measured samples')
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def docker(*args):
    return subprocess.check_output(['docker', *args], timeout=60, text=True)


def memory_bytes(text):
    # Docker's formatter uses binary or decimal suffixes, independently of locale.
    units = {'B': 1, 'kB': 1000, 'KB': 1000, 'KiB': 1024, 'MB': 10 ** 6, 'MiB': 1024 ** 2,
        'GB': 10 ** 9, 'GiB': 1024 ** 3, 'TB': 10 ** 12, 'TiB': 1024 ** 4}
    value = text.split('/')[0].strip()
    for suffix in sorted(units, key=len, reverse=True):
        if value.endswith(suffix):
            return int(float(value[:-len(suffix)]) * units[suffix])
    raise ValueError('unknown Docker memory unit')


def resources(project, lab):
    ids = docker('ps', '-q', '--filter', 'label=com.docker.compose.project=' + project).split()
    if not ids:
        raise ValueError('the owned release stack is absent')
    ids.append(docker('inspect', '--format', '{{.Id}}', lab).strip())
    stats = [json.loads(line) for line in docker('stats', '--no-stream', '--format', '{{json .}}', *ids).splitlines()]
    rows = []
    for stat in stats:
        # The fixture's images contain a shell; count the descriptors of PID 1 only, consistently.
        raw = docker('exec', stat['ID'], 'sh', '-c', 'find /proc/1/fd -mindepth 1 -maxdepth 1 2>/dev/null | wc -l').strip()
        rows.append({'id': stat['ID'], 'name': stat['Name'], 'bytes': memory_bytes(stat['MemUsage']),
            'cpu_percent': float(stat['CPUPerc'].rstrip('%')), 'pids': int(stat['PIDs']), 'fds': int(raw)})
    return {'at': time.time(), 'containers': rows, 'bytes': sum(row['bytes'] for row in rows),
        'fds': sum(row['fds'] for row in rows), 'pids': sum(row['pids'] for row in rows)}


def append(path, record):
    with path.open('a') as output:
        output.write(json.dumps(record, sort_keys=True) + '\n')
