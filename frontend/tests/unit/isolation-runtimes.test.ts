// @vitest-environment node
/** What the screens tell of the runtime an environment runs under: which limits touch an environment, which runtimes a template and a place let a person choose, and that a host naming runtimes without declaring them is still offered. */
import { describe, expect, it } from 'vitest';

import { allowedDefault, defaultRuntime, runtimeOffer } from '@/features/resources/isolation/RuntimeChoice';
import { MOUNTS_SUBJECT, allowedRuntimes, capitalize, classNamed, classesOfDriver, limitTouches, limitsTouching, moveRemovesSoftware, policyRefusal, templateRuntime, templateSubjects } from '@/features/resources/isolation/runtimes';
import type { Host, RuntimeClass, RuntimeLimit, Template } from '@/generated/api-types';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

const limit = (key: string, touches: string[] = [], todo?: string): RuntimeLimit => ({ key, touches, why: `${key} why`, ...(todo === undefined ? {} : { todo }) });

const gvisor: RuntimeClass = {
  name: 'runsc',
  title: 'gVisor',
  aliases: ['gvisor'],
  boundary: 'user_space_kernel',
  capabilities: {},
  live_changes: { gateway_given: [], resources: false, mounts: false, capabilities: [] },
  network_egress: [],
  recreate_preserves_files: false,
  keeps_software_on_stop: false,
  memory_overhead_mb: 64,
  limits: [limit('live_changes', [], 'a mount or device change makes the environment again'), limit('file_events', [MOUNTS_SUBJECT]), limit('kvm', ['kvm']), limit('docker_inside_network', ['docker_inside'])],
};

const hostOf = (...runtimes: RuntimeClass[]): Pick<Host, 'drivers'> => ({ drivers: [{ name: 'docker', runtime_classes: runtimes.map((entry) => entry.name), runtimes } as unknown as Host['drivers'][number]] });

describe('software carried by a move', () => {
  const from = { id: 'source', name: 'Source', address: 'source.example' };

  it('warns when a hosted unit has no kept image, including a legacy plan without the field', () => {
    expect(moveRemovesSoftware({ from, files: { kept_image_bytes: null } })).toBe(true);
    expect(moveRemovesSoftware({ from, files: {} })).toBe(true);
  });

  it('keeps software when an image is carried, including a measured empty image', () => {
    expect(moveRemovesSoftware({ from, files: { kept_image_bytes: 0 } })).toBe(false);
    expect(moveRemovesSoftware({ from, files: { kept_image_bytes: 1024 } })).toBe(false);
  });

  it('does not claim software is removed from a machine directory', () => {
    expect(moveRemovesSoftware({ from: null, files: {} })).toBe(false);
  });
});

describe('the limits that touch an environment', () => {
  it('are those that name nothing, or something the environment has, can be given, or mounts', () => {
    const limits = gvisor.limits;
    expect(limitsTouching(limits, []).map((entry) => entry.key)).toEqual(['live_changes']);
    expect(limitsTouching(limits, ['kvm']).map((entry) => entry.key)).toEqual(['live_changes', 'kvm']);
    expect(limitsTouching(limits, ['docker_inside', MOUNTS_SUBJECT]).map((entry) => entry.key)).toEqual(['live_changes', 'file_events', 'docker_inside_network']);
    expect(limitTouches(limit('x', ['a', 'b']), ['b'])).toBe(true);
    expect(limitsTouching(undefined, ['kvm'])).toEqual([]);
  });

  it('are those of a template: what it holds from the start or allows later, and the mounts only where there are some', () => {
    const spec = { base_capabilities: ['kvm'], allowed_capabilities: ['docker_inside', 'kvm'] };
    expect(templateSubjects(spec)).toEqual(['kvm', 'docker_inside']);
    expect(templateSubjects(spec, true)).toEqual(['kvm', 'docker_inside', MOUNTS_SUBJECT]);
    expect(templateSubjects(null)).toEqual([]);
  });

  it('are shown as sentences that start with a capital', () => {
    expect(capitalize('a mount or device change makes the environment again')).toBe('A mount or device change makes the environment again');
    expect(capitalize('')).toBe('');
  });
});

describe('the runtimes a person can choose', () => {
  it('are named by their name or by any other name the daemon knows them by', () => {
    expect(classNamed([gvisor], 'gvisor')?.title).toBe('gVisor');
    expect(classNamed([gvisor], 'runsc')?.title).toBe('gVisor');
    expect(classNamed([gvisor], 'kata')).toBeUndefined();
    expect(classNamed([gvisor], '')).toBeUndefined();
  });

  it('are each host\'s classes once, and a runtime a host only names is offered with the default declaration', () => {
    const named = { drivers: [{ name: 'docker', runtime_classes: ['runc', 'runsc', 'crun'] } as unknown as Host['drivers'][number]] };
    expect(classesOfDriver([hostOf(gvisor), hostOf(gvisor)], 'docker').map((entry) => entry.name)).toEqual(['runsc']);
    expect(classesOfDriver([hostOf(gvisor)], 'kubernetes')).toEqual([]);
    const template = { driver: 'docker', spec: { runtime: 'runsc', allowed_runtimes: ['runsc', 'crun'] } } as unknown as Template;
    const offer = runtimeOffer(template, [hostOf(gvisor), named]);
    expect(offer.classes.map((entry) => entry.name)).toEqual(['runsc', 'runc', 'crun']);
    expect(offer.classes.find((entry) => entry.name === 'crun')?.boundary).toBe('shared_kernel');
    expect(offer.own).toBe('runsc');
    expect(offer.permitted).toEqual(['runsc', 'crun']);
    expect(defaultRuntime(offer)).toBe('runsc');
  });

  it('start from the template\'s own, else the first the host offers', () => {
    const template = { driver: 'docker', spec: {} } as unknown as Template;
    expect(defaultRuntime(runtimeOffer(template, [hostOf(gvisor)]))).toBe('runsc');
    expect(templateRuntime({ runtime: '' })).toBeNull();
    expect(allowedRuntimes({ allowed_runtimes: ['a', 3, ''] })).toEqual(['a']);
  });

  it('start from the first the place allows where the template\'s own is refused, and from the template\'s own otherwise', () => {
    const template = { driver: 'docker', spec: { runtime: 'runc' } } as unknown as Template;
    const runc: RuntimeClass = { ...gvisor, name: 'runc', title: 'runc', boundary: 'shared_kernel', limits: [] };
    const offer = runtimeOffer(template, [hostOf(runc, gvisor)]);
    expect(allowedDefault(offer, null)).toBeNull();
    expect(allowedDefault(offer, { '*': 'allow' })).toBeNull();
    expect(allowedDefault(offer, { runc: 'forbid' })).toBe('runsc');
    expect(allowedDefault(offer, { runsc: 'require' })).toBe('runsc');
    expect(allowedDefault(offer, { '*': 'forbid' })).toBeNull();
    const limited = runtimeOffer({ driver: 'docker', spec: { runtime: 'runc', allowed_runtimes: ['runc'] } } as unknown as Template, [hostOf(runc, gvisor)]);
    expect(allowedDefault(limited, { runc: 'forbid' }), 'the template does not permit gVisor either').toBeNull();
  });

  it('are refused by a place that forbids them or requires another', () => {
    expect(policyRefusal({ '*': 'allow', runsc: 'forbid' }, 'runsc')).toBe('forbidden');
    expect(policyRefusal({ '*': 'forbid', runc: 'allow' }, 'runc')).toBeNull();
    expect(policyRefusal({ '*': 'allow', kata: 'require' }, 'runc')).toBe('required_elsewhere');
    expect(policyRefusal({ '*': 'allow', kata: 'require' }, 'kata')).toBeNull();
    expect(policyRefusal(null, 'runc')).toBeNull();
  });
});

describe('the words of the pair and its limits', () => {
  const walk = (value: unknown, path: string[] = []): string[] =>
    value !== null && typeof value === 'object' ? Object.entries(value).flatMap(([key, inner]) => walk(inner, [...path, key])) : [path.join('.')];

  it('are in both languages with the same keys', () => {
    const wording = (catalogue: unknown): string[] => walk((catalogue as { isolation: unknown }).isolation).sort();
    expect(wording(chinese)).toEqual(wording(english));
  });

  it('carry every boundary the contract names', () => {
    const boundaries = Object.keys((english as unknown as { isolation: { boundary: Record<string, string> } }).isolation.boundary).sort();
    expect(boundaries).toEqual(['none', 'own_kernel', 'shared_kernel', 'user_space_kernel']);
  });
});
