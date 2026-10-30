"""Measure the built related-list endpoint and agent tool on an isolated native search corpus; never build."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from datetime import datetime, timezone
from urllib.parse import urlparse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--programs', type=Path, required=True, help='Cargo artifact JSON or suite executable manifest')
    parser.add_argument('--profile', choices=['debug', 'release'], required=True)
    parser.add_argument('--database-url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--prepare', action='store_true', help='derive item vectors and real typed-reference neighborhoods first')
    parser.add_argument('--timeout-secs', type=int, default=10800, help='three hours permits the earlier native-corpus preparation plus its item graph')
    args = parser.parse_args()
    if args.timeout_secs <= 0:
        parser.error('timeout-secs must be positive')
    address = urlparse(args.database_url)
    if address.hostname != '127.0.0.1' or address.port == 41532 or not 41000 <= (address.port or 0) <= 41999 or not address.path.startswith('/orchprobe_search_'):
        parser.error('name an isolated loopback orchprobe_search_ database in the probe port range')
    raw = args.programs.read_text()
    try:
        messages = json.loads(raw)
    except json.JSONDecodeError:
        messages = [json.loads(line) for line in raw.splitlines() if line.startswith('{')]
    programs = []
    for item in messages:
        if item.get('name') == 'related_scale' and item.get('exe'):
            programs.append(Path(item['exe']))
        elif item.get('reason') == 'compiler-artifact' and item.get('target', {}).get('name') == 'related_scale' and item.get('executable'):
            programs.append(Path(item['executable']))
    if len(set(programs)) != 1:
        parser.error('manifest must name exactly one compiled related_scale test target')
    program = programs[0]
    if args.profile not in program.parts:
        parser.error('compiled artifact path does not match the requested profile')
    args.output.mkdir(parents=True, exist_ok=True)
    environment = {
        'started_at': datetime.now(timezone.utc).isoformat(), 'profile': args.profile,
        'build_performed': False, 'cpu_count': os.cpu_count(), 'load_started': os.getloadavg(),
        'initial_cache_state': 'unspecified; fixture and permission reads precede warmups',
    }
    cases = ['prepare_related_vectors_and_reference_neighborhoods'] if args.prepare else []
    cases.append('a_million_document_corpus_reports_related_latency')
    root = Path(__file__).resolve().parents[2]
    code = 0
    with (args.output / 'output.log').open('w') as output:
        for case in cases:
            env = dict(os.environ, ORCH_RELATED_SCALE_URL=args.database_url,
                       ORCH_RELATED_SCALE_REPORT=str((args.output / f'{case}.json').resolve()), RUST_MIN_STACK='16777216')
            try:
                result = subprocess.run([str(program), '--ignored', '--exact', case, '--nocapture', '--test-threads=1'],
                                        cwd=root / 'crates/control/api', env=env, stdout=output, stderr=subprocess.STDOUT, timeout=args.timeout_secs)
            except subprocess.TimeoutExpired:
                code = 124
                output.write(f'\n{case} exceeded the requested runner timeout\n')
                break
            output.flush()
            code = result.returncode
            if code:
                break
            report = json.loads(Path(env['ORCH_RELATED_SCALE_REPORT']).read_text())
            if report.get('profile') != args.profile:
                output.write(f'\ncompiled profile differs from requested {args.profile}\n')
                code = 2
                break
    environment.update(finished_at=datetime.now(timezone.utc).isoformat(), load_finished=os.getloadavg(), exit=code)
    (args.output / 'environment.json').write_text(json.dumps(environment, indent=2) + '\n')
    print(f'DONE exit={code}', flush=True)
    return code


if __name__ == '__main__':
    sys.exit(main())
