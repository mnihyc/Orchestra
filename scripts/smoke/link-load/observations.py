"""Verify actual QA process/phase evidence without combining different clocks or hiding losses."""
from qa import ROLES, validate_snapshot
from faults import classify

COUNTERS = ('queued_messages', 'queued_payload_bytes', 'queued_unknown_sizes', 'in_flight_messages',
    'in_flight_payload_bytes', 'in_flight_unknown_sizes', 'enqueue_attempts', 'selected', 'sent', 'processed',
    'failed_enqueues', 'selected_without_completion', 'discarded_on_receiver_close', 'unknown_sizes_seen',
    'peak_messages', 'peak_payload_bytes', 'peak_retained_payload_bytes')
MONOTONIC = ('enqueue_attempts', 'selected', 'sent', 'processed', 'failed_enqueues',
    'selected_without_completion', 'discarded_on_receiver_close', 'unknown_sizes_seen',
    'peak_messages', 'peak_payload_bytes', 'peak_retained_payload_bytes')
ZERO = ('queued_messages', 'queued_payload_bytes', 'queued_unknown_sizes', 'in_flight_messages',
    'in_flight_payload_bytes', 'in_flight_unknown_sizes', 'failed_enqueues', 'selected_without_completion',
    'discarded_on_receiver_close', 'unknown_sizes_seen')


def number(value):
    return type(value) is int and value >= 0


def queues(record, drained):
    observed = record.get('observations', {})
    rows = observed.get('queues')
    if observed.get('overflowed') is not False or not isinstance(rows, list) or not rows:
        raise ValueError('missing or overflowed queue observations')
    for row in rows:
        if not isinstance(row.get('label'), str) or not all(number(row.get(key)) for key in COUNTERS):
            raise ValueError('queue payload/message observations are missing or invalid')
        if any(row.get(flag) is not False for flag in ('inconsistent', 'indeterminate_close_race')):
            raise ValueError('queue accounting is inconsistent or raced closure')
        if type(row.get('receiver_closed')) is not bool:
            raise ValueError('queue receiver lifecycle is missing')
        retirement = row.get('drained_retirement')
        if not row['receiver_closed'] and retirement is not None:
            raise ValueError('an active queue cannot claim a receiver retirement')
        if drained and (any(row[key] != 0 for key in ZERO)
                or row['selected'] != row['sent'] + row['processed'] or row['enqueue_attempts'] != row['selected']):
            raise ValueError('a discarded, failed or unfinished queue is not a drain')
        if drained and row['receiver_closed'] and (not isinstance(retirement, dict)
                or not all(number(value) for value in retirement.values())
                or retirement != {key: row[key] for key in ('enqueue_attempts', 'selected', 'sent', 'processed')}):
            raise ValueError('a retired queue needs immutable clean-at-close counters, unchanged since closure')
    if drained and (not any(row['receiver_closed'] is False for row in rows)
            or any(row['receiver_closed'] and not any(not active['receiver_closed'] and active['label'] == row['label'] for active in rows) for row in rows)):
        raise ValueError('closed history needs actual active replacements for a positive open phase')
    return rows


def checkpoint(record, preflight, pinned, phase, boundary):
    request = record.get('request', {})
    if request.get('run') != pinned['run'] or request.get('phase') != phase or request.get('boundary') != boundary:
        raise ValueError('the checkpoint is not this actual run/phase boundary')
    for role in ROLES:
        process = record.get('processes', {}).get(role, {})
        validate_snapshot(process, preflight, pinned['run'], pinned['machine'], role, pinned['instances'][role])
        if process.get('checkpoint') != request:
            raise ValueError('the process did not acknowledge the exact phase request')
        if not number(process.get('elapsed_micros')):
            raise ValueError('the process omitted its local monotonic observation time')
        queues(process, boundary != 'start')
        if boundary != 'start' and process.get('open_drained') is not True:
            raise ValueError('the process did not observe an open drain')
    return record['processes']


def unique_commands(record, key):
    rows = record.get('observations', {}).get('commands')
    if not isinstance(rows, list):
        raise ValueError('command observations are missing')
    result = {}
    for row in rows:
        identity = row.get(key)
        if identity is None:
            continue
        if identity in result:
            raise ValueError('command/request identity is not unique')
        result[identity] = row
    return result


def verify_phase(client, before, after, preflight, pinned, phase):
    if client.get('profile') != ('floor' if phase == 'floor' else 'profile'):
        raise ValueError('QA phase does not match the actual client profile')
    start = checkpoint(before, preflight, pinned, phase, 'start')
    end = checkpoint(after, preflight, pinned, phase, 'drained')
    peaks = {}
    for role in ROLES:
        if end[role]['elapsed_micros'] <= start[role]['elapsed_micros']:
            raise ValueError('phase boundaries are not ordered in the same process clock')
        older, current = queues(start[role], False), queues(end[role], True)
        if len(current) < len(older):
            raise ValueError('queue lifetime evidence disappeared during the phase')
        for index, row in enumerate(older):
            if (current[index]['label'] != row['label'] or any(current[index][field] < row[field] for field in MONOTONIC)
                    or (row['receiver_closed'] and (not current[index]['receiver_closed']
                        or current[index].get('drained_retirement') != row.get('drained_retirement')))):
                raise ValueError('append-only queue registry or cumulative counters changed identity')
        peaks[role] = [{'label': row['label'], 'lifecycle': 'retired_drained' if row['receiver_closed'] else 'active_drained',
            'drained_retirement': row.get('drained_retirement'), 'peak_messages': row['peak_messages'],
            'peak_logical_payload_bytes': row['peak_retained_payload_bytes']} for row in current]
    api = unique_commands(end['api'], 'request_id')
    gateway = unique_commands(end['gateway'], 'id')
    loaded = [sample for sample in client['mixed']['probes'] if sample.get('active_bulk_streams', 0) > 0]
    scopes = [classify(sample) for sample in loaded]
    if 'steady' not in scopes or 'unobserved' in scopes or any(sample['fault_before']['run'] != pinned['run'] for sample in loaded):
        raise ValueError('the phase lacks actual same-run steady/fault observations during load')
    seen, timings = set(), []
    for sample, fault_scope in zip(loaded, scopes):
        request = sample.get('request_id')
        if not request or request in seen or request not in api:
            raise ValueError('public probe lacks one actual API request/command correlation')
        seen.add(request)
        command = api[request]; carrier = gateway.get(command.get('id'))
        if not carrier or command.get('machine_id') != pinned['machine'] or carrier.get('machine_id') != pinned['machine'] \
                or not command.get('gateway_session_id') or command['gateway_session_id'] != carrier.get('gateway_session_id'):
            raise ValueError('API and gateway did not observe the same actual command/machine/session')
        api_seen, carrier_seen = command.get('seen'), carrier.get('seen')
        if not (isinstance(api_seen, list) and isinstance(carrier_seen, list) and len(api_seen) == len(carrier_seen) == 4
                and all(number(value) for value in api_seen + carrier_seen)):
            raise ValueError('handoff/ACK counts are not actual nonnegative integer observations')
        if fault_scope == 'steady':
            if api_seen != [1, 0, 0, 1] or carrier_seen != [0, 1, 1, 0]:
                raise ValueError('missing or duplicate steady handoff/ACK observations remain incomplete')
        elif not (isinstance(api_seen, list) and isinstance(carrier_seen, list) and len(api_seen) == len(carrier_seen) == 4
                and all(number(value) for value in api_seen + carrier_seen)
                and api_seen[0] > 0 and api_seen[3] > 0 and api_seen[1:3] == [0, 0]
                and carrier_seen[1] > 0 and carrier_seen[2] > 0 and carrier_seen[0] == carrier_seen[3] == 0):
            raise ValueError('fault-spanning call still requires actual correlated completion/ACK observations')
        if not number(command.get('api_ack_micros')) or not number(carrier.get('carrier_ack_micros')) \
                or command.get('carrier_ack_micros') is not None or carrier.get('api_ack_micros') is not None:
            raise ValueError('ACK evidence is not a duration pair from each own process')
        if command['api_ack_micros'] > end['api']['elapsed_micros'] - start['api']['elapsed_micros'] \
                or carrier['carrier_ack_micros'] > end['gateway']['elapsed_micros'] - start['gateway']['elapsed_micros']:
            raise ValueError('ACK duration lies outside its own process phase interval')
        if phase != 'floor' and fault_scope == 'steady' and (carrier['carrier_ack_micros'] > 1_000_000 or command['api_ack_micros'] > 1_000_000):
            raise ValueError('the actual recommended-profile API or carrier ACK exceeded 1000ms')
        timings.append({'request_id': request, 'command_id': command['id'],
            'fault_status': fault_scope, 'handoff_ack_counts': {'api': api_seen, 'gateway': carrier_seen},
            'api_ack_micros': command['api_ack_micros'], 'carrier_ack_micros': carrier['carrier_ack_micros']})
    if not timings:
        raise ValueError('no actual command overlapped bulk traffic')
    return {'qa_phase_observed': True, 'positive_open_drain': True, 'own_clock_ack_samples': timings,
        'command_ack_budget': 'not_required_at_floor' if phase == 'floor' else 'observed_exact_api_and_carrier_ack_for_steady_profile',
        'whole_session_queue_bound': 'observed_workload_conservation_only',
        'queue_measurement': 'observed logical peaks and open drain; no RSS or mathematical capacity claim',
        'queue_peaks': peaks, 'remaining_observers': [], 'complete_wss_release_gate': False}


def verify_final(pre_stop, final, preflight, pinned):
    opened = checkpoint(pre_stop, preflight, pinned, 'shutdown', 'pre_stop')
    for role in ROLES:
        # At least this actual workload used every selected local/data queue family.
        for label in {'api': ['api.terminal_out', 'controller.down', 'controller.notices'],
                'gateway': ['gateway.backplane', 'link.data'], 'connector': ['machine.inputs', 'link.data']}[role]:
            if not any(row['label'] == label and row['selected'] > 0 for row in opened[role]['observations']['queues']):
                raise ValueError('the workload did not exercise a required observed queue family')
        stopped = final.get('processes', {}).get(role, {})
        validate_snapshot({**stopped, 'state': 'running'}, preflight, pinned['run'], pinned['machine'], role, pinned['instances'][role], require_active=False)
        if stopped.get('state') != 'finished' or stopped.get('elapsed_micros', -1) <= opened[role]['elapsed_micros']:
            raise ValueError('process shutdown did not follow its positive open-drain observation')
        if stopped.get('observations', {}).get('overflowed') is not False:
            raise ValueError('the process recorder lost observations during shutdown')
    if final.get('complete') is not True or final.get('failures') != []:
        raise ValueError('QA process cleanup is incomplete')
