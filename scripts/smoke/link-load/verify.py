#!/usr/bin/env python3
"""Join the two ends' independent byte evidence, including the deliberately abandoned stream."""
import argparse
import json
from pathlib import Path
from peer import digest


def verify(client, peer):
    if not client.get('passed') or not client.get('streams'):
        raise ValueError('the actual client did not complete')
    seeds = [row['seed'] for row in client['streams']]
    if len(set(seeds)) != len(seeds):
        raise ValueError('two connections reused a stream identity')
    for row in client['streams']:
        other = peer[row['seed']]
        if not other['closed'] or other.get('send_stuck'):
            raise ValueError('a cut or half-close left its peer stuck')
        if row['cut']:
            if not 0 < row['download_bytes'] < row['expected_bytes']:
                raise ValueError('the deliberate cut was not an interrupted stream')
            if other['upload_bytes'] > row['upload_bytes'] or other['download_bytes'] < row['download_bytes']:
                raise ValueError('the abandoned stream has impossible byte counts')
            continue
        count, seed = row['expected_bytes'], row['seed']
        if row['upload_bytes'] != count or row['download_bytes'] != count or other['upload_bytes'] != count or other['download_bytes'] != count:
            raise ValueError('a direction was shortened or duplicated')
        if other['upload_sha256'] != digest(seed, 'upload', count) or row['download_sha256'] != digest(seed, 'download', count):
            raise ValueError('cross-stream, cross-direction or changed bytes')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--peer', type=Path, required=True)
    args = parser.parse_args()
    client = json.loads(args.client.read_text())
    peer = {row['seed']: json.loads((args.peer / (row['seed'] + '.json')).read_text()) for row in client['streams']}
    verify(client, peer)


if __name__ == '__main__':
    main()
