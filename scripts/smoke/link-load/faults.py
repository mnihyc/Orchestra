"""Classify the existing deliberate blackout without comparing process clocks."""


def classify(sample, history=None):
    before, after = sample.get('fault_before'), sample.get('fault_after')
    if before is None or after is None:
        return 'unobserved'
    for marker in (before, after):
        if not isinstance(marker, dict) or type(marker.get('generation')) is not int \
                or not 0 <= marker['generation'] <= 2 or type(marker.get('active')) is not bool \
                or not isinstance(marker.get('run'), str):
            raise ValueError('invalid bounded fault-generation observation')
        if history is not None and (marker['generation'] >= len(history) or marker != history[marker['generation']]):
            raise ValueError('the probe observed another fault history')
    if before['run'] != after['run'] or after['generation'] < before['generation']:
        raise ValueError('fault generation changed run or moved backwards')
    if before['generation'] == after['generation'] and before['active'] != after['active']:
        raise ValueError('fault state changed without a new generation')
    return 'steady' if before == after and before['active'] is False else 'intentional_fault'


def verify_history(history, run, blackout=True):
    if not blackout:
        if history != [{'run': run, 'generation': 0, 'active': False}]:
            raise ValueError('the short placement probe must retain its actual steady profile')
        return
    if history != [{'run': run, 'generation': number, 'active': number == 1} for number in range(3)]:
        raise ValueError('the actual one-blackout fault history is incomplete or changed')
