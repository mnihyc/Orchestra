"""Reviewed workload allocation and harness budgets; these are not product latency promises."""
import argparse
import asyncio

MIB = 1024 * 1024
LARGE = 100 * MIB
PRIMARY = 'connector'
PHASES = ('concurrent', 'cut', 'reconnect', 'floor')
PHASE_BUDGET_SECONDS = 7200  # Retained ordinary fixture watchdog.
FLOOR_BUDGET_SECONDS = 21600  # Explicit six-hour QA work budget; never a floor SLA.


def phase(placement, name):
    if placement not in ('client', 'connector', 'backplane') or name not in PHASES:
        raise ValueError('unknown attributed workload phase')
    count = LARGE if name == 'cut' or (placement == PRIMARY and name in ('concurrent', 'floor')) else MIB
    return {'streams': 1, 'bytes': count, 'cut': name == 'cut', 'stall': name == 'concurrent',
        'budget_seconds': FLOOR_BUDGET_SECONDS if placement == PRIMARY and name == 'floor' else PHASE_BUDGET_SECONDS}


def payload_floor_seconds(count, profile):
    return count * 8 / (75000 if profile == 'floor' else 750000)


class HarnessBudgetExpired(TimeoutError):
    """The fixture's observation budget ended; this alone says nothing about a product defect."""


async def within_budget(awaitable, seconds):
    task = asyncio.create_task(awaitable)
    try:
        done, _ = await asyncio.wait({task}, timeout=seconds)
        if task not in done:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
            raise HarnessBudgetExpired('the harness work budget ended')
        return task.result()  # An inner TimeoutError stays a separate observation failure.
    finally:
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)


def failure(error, seconds, progress):
    budget = isinstance(error, HarnessBudgetExpired)
    return {'passed': False, 'failure': type(error).__name__,
        'failure_domain': 'harness_budget' if budget else 'observation',
        'assessment': 'incomplete_no_product_verdict' if budget else 'requires_diagnosis',
        'harness_budget_seconds': seconds, 'last_payload_observations': list(progress.values())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('placement', choices=('client', 'connector', 'backplane'))
    parser.add_argument('phase', choices=PHASES)
    args = parser.parse_args()
    item = phase(args.placement, args.phase)
    values = ['--streams', str(item['streams']), '--bytes', str(item['bytes']),
        '--harness-budget-seconds', str(item['budget_seconds'])]
    if args.phase == 'floor': values += ['--profile', 'floor']
    if item['cut']: values += ['--cut']
    if item['stall']: values += ['--stall']
    print('\n'.join(values))


if __name__ == '__main__':
    main()
