/** What the screens of resources read out of what the server says, and write back into it: the facts a machine reports, a harness's own declaration, a template's recipe, the endpoints of a provider, a host's figures, and the words of an address that narrows a list. */
import type { TFunction } from 'i18next';
import { describe, expect, it } from 'vitest';

import { humanize, phrase, readDescriptor, versionPin } from '@/features/resources/agents/descriptor';
import { declaredShare, usedShare } from '@/features/resources/hosts/words';
import { allowedRoots, formatRate, readMachineFacts, registrableHarnesses } from '@/features/resources/machines/facts';
import { commandFor, guessOs } from '@/features/resources/machines/Enrollment';
import { commandValue, policyCommand } from '@/features/resources/machines/Policy';
import { draftOf, endpointFault, upstreamOf } from '@/features/resources/providers/Endpoints';
import { holding, howItRuns, isBusy, threadCounts } from '@/features/resources/environments/words';
import { matches, oneOf, readText } from '@/features/resources/search';
import { settingValue } from '@/features/resources/SettingsTable';
import { amountsOf, confinementOf, execOffers, hasAmounts, imageSource, readSpec, schemaFields, specSchema, specText, starterSpec } from '@/features/resources/templates/spec';
import type { EnrollmentToken, ProviderSetupProtocol } from '@/generated/api-types';
import { initialValues, missingFields, textValues } from '@/ui/FieldForm';

/** What a harness's adapter says of itself, cut down to what the screens read. */
const declared = {
  harness: { name: 'Codex', status: 'supported', version: '0.162.1', supported_range: '>=1.0.0' },
  parameters: [
    { key: 'reasoning_effort', type: 'enum', apply: 'next_turn', title: 'Reasoning effort', scopes: ['org', 'thread'], default: { from_model: 'default_effort' }, options: { from_model: 'efforts' }, description: 'How hard it thinks.' },
    { key: 'verbosity', type: 'enum', scopes: [], default: 'normal', options: [{ label: 'Terse', value: 'terse' }, { label: 'Normal', value: 'normal' }] },
    { type: 'enum', title: 'A parameter without a key is left out' },
  ],
  arrangements: [{ type: 'separated', executor: 'orch-exec', tool_sets: ['plain'] }, { type: 'key_separated' }, { executor: 'no type' }],
  capabilities: { plan: true, effort: 'turn' },
  sandbox_modes: ['read_only', 'open'],
  approval_modes: ['ask'],
  adapter_version: 1,
};

/** A translation that says its key and what it was given, so that a test sees which sentence was chosen. */
const t = ((key: string, options?: Record<string, unknown>) => (options === undefined ? key : `${key} ${JSON.stringify(options)}`)) as unknown as TFunction;

describe('the facts a connector reports of its machine', () => {
  it('are read with care: what is missing or of the wrong kind is left out', () => {
    const facts = readMachineFacts({
      hostname: 'ada-laptop',
      cpus: 8,
      memory_bytes: '16 GB',
      harnesses: [{ kind: 'codex', path: '/usr/bin/codex', version: '0.162.1', daemon: true }, { path: 'no kind' }, 'nonsense'],
      adapters: [{ kind: 'codex', builtin: true }],
      rates: { up_bps: 1_000_000, down_bps: 5_000_000 },
      policy: { 'environment.roots': ['/home/ada/work', 3] },
    });
    expect(facts.hostname).toBe('ada-laptop');
    expect(facts.cpus).toBe(8);
    expect(facts.memoryBytes).toBeNull();
    expect(facts.harnesses).toEqual([{ kind: 'codex', path: '/usr/bin/codex', version: '0.162.1', daemon: true }]);
    // A link is something only when all three of its figures are there.
    expect(facts.link).toBeNull();
    expect(allowedRoots(facts)).toEqual(['/home/ada/work']);
    expect(readMachineFacts(null).harnesses).toEqual([]);
  });

  it('say which harnesses a profile can be made for: those found there that its connector has an adapter for', () => {
    const facts = readMachineFacts({
      harnesses: [{ kind: 'codex' }, { kind: 'claude-code' }],
      adapters: [{ kind: 'codex' }],
    });
    expect(registrableHarnesses(facts).map((harness) => harness.kind)).toEqual(['codex']);
  });

  it('give a rate in the unit people read it in', () => {
    expect(formatRate(80, 'en')).toBe('80 bit/s');
    expect(formatRate(12_000, 'en')).toBe('12 kbit/s');
    expect(formatRate(700_000, 'en')).toBe('700 kbit/s');
    expect(formatRate(7_300_000, 'en')).toBe('7.3 Mbit/s');
    expect(formatRate(-5, 'en')).toBe('0 bit/s');
  });
});

describe('the command that enrolls a machine', () => {
  const token: EnrollmentToken = {
    token: 'oenr_x',
    expires_at: '2026-10-12T12:00:00Z',
    commands: { linux: 'curl | sh -s -- --token x', macos: 'curl | sh -s -- --token x', windows: '& iwr -Token x' },
  };

  it('is the one for the system of the browser first', () => {
    expect(guessOs('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')).toBe('windows');
    expect(guessOs('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)')).toBe('macos');
    expect(guessOs('Mozilla/5.0 (X11; Linux x86_64)')).toBe('linux');
    expect(guessOs('')).toBe('linux');
  });

  it('shows the exact issued command instead of adding permissions to an existing token', () => {
    expect(commandFor(token, 'linux')).toBe('curl | sh -s -- --token x');
    expect(commandFor(token, 'windows')).toBe('& iwr -Token x');
    const selected = { ...token, commands: { ...token.commands, linux: 'install --roles environment,provider --delegate environment --dedicated' } };
    expect(commandFor(selected, 'linux')).toBe(selected.commands.linux);
  });
});

describe('the command that sets a limit on the machine itself', () => {
  it('writes a value as the command line reads it', () => {
    expect(commandValue(true)).toBe('true');
    expect(commandValue(30)).toBe('30');
    expect(commandValue('workspace')).toBe('workspace');
    expect(commandValue(['/home/ada/work', '/srv'])).toBe(`'["/home/ada/work","/srv"]'`);
    expect(commandValue("it's")).toBe(`'"it'\\''s"'`);
  });

  it('keeps the displayed value explicitly when run on the machine', () => {
    expect(policyCommand('environment.terminal', false)).toBe('orch-connector policy set environment.terminal false');
    expect(policyCommand('agent.max_sandbox', 'open')).toBe('orch-connector policy set agent.max_sandbox open');
  });
});

describe('what a harness declares of itself', () => {
  it('is nothing at all until the harness has reported', () => {
    expect(readDescriptor({}).declared).toBe(false);
    expect(readDescriptor(null).parameters).toEqual([]);
  });

  it('is read into its parameters, arrangements and capabilities', () => {
    const view = readDescriptor(declared);
    expect(view.declared).toBe(true);
    expect(view.harness).toEqual({ name: 'Codex', status: 'supported', version: '0.162.1', range: '>=1.0.0' });
    expect(view.parameters.map((parameter) => parameter.key)).toEqual(['reasoning_effort', 'verbosity']);
    expect(view.parameters[0]).toMatchObject({ title: 'Reasoning effort', scopes: ['org', 'thread'], default: '← default_effort', options: '← efforts' });
    expect(view.parameters[1]?.options).toBe('Terse, Normal');
    expect(view.arrangements).toEqual([{ type: 'separated', executor: 'orch-exec', toolSets: ['plain'] }, { type: 'key_separated', executor: null, toolSets: [] }]);
    expect(view.capabilities).toContainEqual({ key: 'plan', value: 'true' });
  });

  it('names a value in a few words and a key as people read it', () => {
    expect(phrase({ from_model: 'efforts' })).toBe('← efforts');
    expect(phrase([{ label: 'Terse', value: 'terse' }, 'plain'])).toBe('Terse, plain');
    expect(phrase(null)).toBe('');
    expect(humanize('fork_native')).toBe('Fork native');
  });

  it('pins a version only where the profile says so', () => {
    expect(versionPin({ harness_version: '0.161.0' })).toBe('0.161.0');
    expect(versionPin({ harness_version: '' })).toBeNull();
    expect(versionPin(null)).toBeNull();
  });
});

describe('the endpoints of a provider', () => {
  const protocol: ProviderSetupProtocol = {
    name: 'openai',
    title: 'OpenAI',
    agents: [],
    endpoint_options: [
      { name: 'organization', label: 'Organization', type: 'text', required: false },
      { name: 'headers', label: 'Secret headers', type: 'secret_map', required: false },
      { name: 'labels', label: 'Labels', type: 'map', required: false },
    ],
  } as unknown as ProviderSetupProtocol;

  it('keep a secret header under its name and send its value beside the endpoint, never in it', () => {
    const draft = draftOf({ protocol: 'openai', base_url: 'https://api.example.test/v1', options: { organization: 'acme', headers: ['x-key'], labels: { team: 'ml' } } }, protocol);
    // What is held has no value to show for a secret; the name stays.
    expect(draft.options.headers).toEqual({ 'x-key': '' });
    draft.options.headers = { 'x-key': 'sekret' };
    const { upstream, secrets } = upstreamOf(draft, protocol);
    expect(upstream).toEqual({ protocol: 'openai', base_url: 'https://api.example.test/v1', options: { organization: 'acme', headers: ['x-key'], labels: { team: 'ml' } } });
    expect(JSON.stringify(upstream)).not.toContain('sekret');
    expect(secrets).toEqual({ 'x-key': 'sekret' });
  });

  it('are faulted in words: no format, no address, an address that is none, an address that would send the key in clear', () => {
    const draft = draftOf({ protocol: 'openai', base_url: '', options: {} }, protocol);
    expect(endpointFault({ ...draft, protocol: '' }, false, t)).toBe('resources.endpoint.protocolEmpty');
    expect(endpointFault(draft, false, t)).toBe('resources.endpoint.addressEmpty');
    expect(endpointFault({ ...draft, baseUrl: 'not an address' }, false, t)).toBe('resources.endpoint.addressInvalid');
    expect(endpointFault({ ...draft, baseUrl: 'ftp://models.example.test' }, false, t)).toBe('resources.endpoint.addressInvalid');
    expect(endpointFault({ ...draft, baseUrl: 'http://models.example.test' }, false, t)).toBe('resources.endpoint.httpRefused');
    expect(endpointFault({ ...draft, baseUrl: 'http://models.example.test' }, true, t)).toBeNull();
    expect(endpointFault({ ...draft, baseUrl: 'https://models.example.test/v1' }, false, t)).toBeNull();
  });
});

describe('the recipe of a template', () => {
  const spec = {
    image: 'orch-env-base:0.1.0',
    runtime: 'runc',
    network: { mode: 'bridge' },
    security: { read_only_rootfs: true, no_new_privileges: false },
    resources: { cpu_millis: 4000, memory_mb: 8192, pids: 4096, disk_gb: 50 },
    needs: { cpu_millis: 1000, memory_mb: 2048 },
    exec: { offers: ['codex-exec-server', 'plain', 3], orch_tools: true },
  };

  it('says where its image comes from: a Dockerfile, an archive, or a reference', () => {
    expect(imageSource({ build: { dockerfile: 'FROM x' }, image: 'ignored' })).toEqual({ kind: 'build' });
    expect(imageSource({ archive: { blob: 'ab' } })).toEqual({ kind: 'archive' });
    expect(imageSource(spec)).toEqual({ kind: 'image', reference: 'orch-env-base:0.1.0' });
    // The recipe of a driver can name nulls for what it does not use.
    expect(imageSource({ build: null, archive: null, image: 'a:1' })).toEqual({ kind: 'image', reference: 'a:1' });
    expect(imageSource({})).toEqual({ kind: 'none' });
    expect(imageSource('nonsense')).toEqual({ kind: 'none' });
  });

  it('says how it confines what runs in it, as far as it says anything', () => {
    expect(confinementOf(spec)).toEqual({ runtime: 'runc', network: 'bridge', readOnlyRoot: true, noNewPrivileges: false });
    expect(confinementOf(null)).toEqual({ runtime: null, network: null, readOnlyRoot: false, noNewPrivileges: false });
  });

  it('states what a unit may use, how far that may be raised, and what a host must have free', () => {
    expect(amountsOf(spec, 'resources')).toEqual({ cpuMillis: 4000, memoryMb: 8192, pids: 4096, diskGb: 50 });
    expect(amountsOf(spec, 'needs')).toEqual({ cpuMillis: 1000, memoryMb: 2048, pids: null, diskGb: null });
    expect(hasAmounts(amountsOf(spec, 'resources_max'))).toBe(false);
  });

  it('lists what its exec node offers, in order, and only words', () => {
    expect(execOffers(spec)).toEqual({ offers: ['codex-exec-server', 'plain'], orchTools: true });
    expect(execOffers({})).toEqual({ offers: [], orchTools: null });
  });

  it('is read back from what was typed only when that is an object', () => {
    expect(readSpec('{ "image": "a:1" }')).toEqual({ image: 'a:1' });
    expect(readSpec('[1]')).toBeNull();
    expect(readSpec('{ "image": ')).toBeNull();
    expect(readSpec('3')).toBeNull();
    expect(specText({ a: 1 })).toBe('{\n  "a": 1\n}');
    expect(specText(undefined)).toBe('{}');
  });

  it('begins from what the driver gives a default for and what its schema requires', () => {
    const schema = specSchema({ spec_schemas: { environment: { type: 'object', required: ['image', 'env'], properties: { image: { type: 'string' }, env: { type: 'object' }, workspace_mount: { type: 'string', default: '/workspace', description: 'Where' }, runtime: { type: 'string' } } } } }, 'environment');
    expect(specSchema({ spec_schemas: {} }, 'provider')).toBeNull();
    expect(starterSpec(schema)).toEqual({ image: '', env: {}, workspace_mount: '/workspace' });
    expect(starterSpec(null)).toEqual({});
    expect(schemaFields(schema)).toEqual([
      { name: 'image', type: 'string', description: '', required: true },
      { name: 'env', type: 'object', description: '', required: true },
      { name: 'workspace_mount', type: 'string', description: 'Where', required: false },
      { name: 'runtime', type: 'string', description: '', required: false },
    ]);
  });
});

describe('the figures of a host', () => {
  it('say how much of what it has is declared, which can pass the whole, and nothing where it reports no capacity', () => {
    expect(declaredShare(4096, 65_536)).toBeCloseTo(6.25);
    expect(declaredShare(80_000, 65_536)).toBeGreaterThan(100);
    expect(declaredShare(1, 0)).toBeNull();
    expect(usedShare({ capacity_gb: 800, free_gb: 540 })).toBeCloseTo(32.5);
    expect(usedShare({ capacity_gb: 0, free_gb: 0 })).toBeNull();
  });
});

describe('an environment in a few words', () => {
  const machine = { id: 'm', name: "Ada's laptop", online: true, placement: 'connected', platform_os: 'linux' };

  it('is a directory on a machine, or what it was made from and where', () => {
    const base = { machine: null, template: null, host: null, workspace_root: '/workspace' } as const;
    expect(howItRuns({ ...base, machine } as never, t)).toBe('environments.how.machine {"root":"/workspace","machine":"Ada\'s laptop"}');
    expect(howItRuns({ ...base, template: { id: 't', name: 'Default', driver: 'docker' } }, t)).toBe('environments.how.template {"template":"Default"}');
    expect(howItRuns({ ...base, template: { id: 't', name: 'Default', driver: 'docker' }, host: { id: 'h', name: 'host-1', address: 'a' } }, t)).toBe('environments.how.templateOn {"template":"Default","host":"host-1"}');
    expect(howItRuns(base as never, t)).toBe('environments.how.none');
  });

  it('shows the highest of what the reader holds, and counts the threads and topics they may see', () => {
    expect(holding(['environment.view', 'environment.use'])).toBe('use');
    expect(holding(['environment.view', 'environment.manage', 'environment.use'])).toBe('manage');
    expect(holding([])).toBe('view');
    const environment = { threads: [{ topic: { id: 'a' } }, { topic: { id: 'a' } }, { topic: { id: 'b' } }], threads_hidden: 2 } as never;
    expect(threadCounts(environment)).toEqual({ threads: 3, topics: 2, hidden: 2 });
    expect(isBusy('provisioning')).toBe(true);
    expect(isBusy('running')).toBe(false);
  });
});

describe('the address of a list', () => {
  it('takes only what a list knows how to narrow by', () => {
    expect(oneOf(['open', 'closed'] as const, 'closed')).toBe('closed');
    expect(oneOf(['open', 'closed'] as const, 'ajar')).toBeUndefined();
    expect(oneOf(['open', 'closed'] as const, 3)).toBeUndefined();
    expect(readText('build')).toBe('build');
    expect(readText('')).toBeUndefined();
    expect(readText('   ')).toBeUndefined();
    expect(readText(['a'])).toBeUndefined();
  });

  it('finds a row by every word typed, in any of its fields, however it is cased', () => {
    expect(matches('build box', 'Build-box', 'acme.ada.build-box')).toBe(true);
    expect(matches('ada box', 'build-box', 'acme.ada.build-box', null)).toBe(true);
    expect(matches('docs', 'build-box', undefined)).toBe(false);
    expect(matches('', 'anything')).toBe(true);
  });
});

describe('the settings a resource resolves', () => {
  it('are read as words, a list, or JSON', () => {
    expect(settingValue('manual')).toBe('manual');
    expect(settingValue(1440)).toBe('1440');
    expect(settingValue(false)).toBe('false');
    expect(settingValue(null)).toBe('');
    expect(settingValue(['a', 'b'])).toBe('a, b');
    expect(settingValue({ a: 1 })).toBe('{"a":1}');
  });
});

describe('the fields a driver declares', () => {
  const fields = [
    { name: 'dockerfile', label: 'The Dockerfile', type: 'secret_text', required: true },
    { name: 'tag', label: 'Tag', type: 'text', required: false, default: 'latest' },
    { name: 'squash', label: 'Squash', type: 'boolean', required: false, default: true },
    { name: 'args', label: 'Build arguments', type: 'map', required: false },
  ] as never;

  it('start at what they declare, are sent as text with what is empty left out, and say which required one is empty', () => {
    const values = initialValues(fields);
    expect(values).toEqual({ dockerfile: '', tag: 'latest', squash: true, args: {} });
    expect(missingFields(fields, values)).toEqual(['dockerfile']);
    values.dockerfile = 'FROM scratch';
    values.args = { A: '1' };
    expect(missingFields(fields, values)).toEqual([]);
    expect(textValues(fields, values)).toEqual({ dockerfile: 'FROM scratch', tag: 'latest', squash: 'true', args: '{"A":"1"}' });
  });
});
