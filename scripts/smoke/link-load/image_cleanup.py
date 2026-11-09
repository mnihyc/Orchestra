#!/usr/bin/env python3
"""Retire only this run's verified netem tag, after its containers are absent."""
import argparse
import json
from pathlib import Path
import subprocess


def docker(*arguments):
    return subprocess.check_output(['docker', *arguments], text=True).strip()


def retire(tag, image_id, run):
    if not image_id:
        if docker('image', 'ls', '--quiet', '--no-trunc', tag):
            raise ValueError('the build left a tag without a recorded image identity; retain it')
        return {'complete': True, 'tag': tag, 'outcome': 'build_created_no_tag'}
    inspected = json.loads(docker('image', 'inspect', tag))
    if len(inspected) != 1:
        raise ValueError('the owned image tag has no unique current identity')
    current = inspected[0]
    labels = (current.get('Config') or {}).get('Labels') or {}
    if current.get('Id') != image_id or labels.get('org.orch.smoke.run') != run:
        raise ValueError('the image tag was replaced or its ownership label differs; retain it')
    if docker('ps', '-aq', '--filter', 'ancestor=' + image_id):
        raise ValueError('a container still references the owned image; retain it')
    # Remove the tag, never force an in-use image or prune shared parent layers.
    docker('image', 'rm', '--no-prune', tag)
    if docker('image', 'ls', '--quiet', '--no-trunc', tag):
        raise ValueError('the owned image tag remains after removal')
    return {'complete': True, 'tag': tag, 'image_id': image_id, 'run': run, 'outcome': 'owned_tag_removed'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--image-id', required=True)
    parser.add_argument('--run', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    status = 0
    try:
        record = retire(args.tag, args.image_id, args.run)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        record = {'complete': False, 'tag': args.tag, 'image_id': args.image_id,
            'fixture_retained': True, 'failure': type(error).__name__, 'detail': str(error)}
        status = 1
    args.output.write_text(json.dumps(record, indent=2) + '\n')
    return status


if __name__ == '__main__':
    raise SystemExit(main())
