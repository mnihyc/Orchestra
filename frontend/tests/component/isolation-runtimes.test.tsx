/** What a person is told about the runtime an environment runs under: the one line under the runtime choice, the limits behind its "?", and the Runtime and Limits rows of an environment. */
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { environmentRuntimeRows, namesRuntime, RuntimeLimitsLine, RuntimePair } from '@/features/resources/isolation/RuntimeInfo';
import { RuntimeChoice } from '@/features/resources/isolation/RuntimeChoice';
import { limitsTouching, templateSubjects } from '@/features/resources/isolation/runtimes';
import type { RuntimeClass, RuntimeIsolation, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const limits = [
  { key: 'live_changes_lost', touches: [], todo: 'a mount or device change makes the environment again', why: 'this runtime cannot change them while it runs; software installed outside the volumes is removed' },
  { key: 'file_events', touches: ['mounts'], todo: 'read the file again instead of waiting for a change event', why: 'files changed outside this environment raise no file events inside it and show up about a second late' },
  { key: 'docker_inside_network', touches: ['docker_inside'], todo: 'run the containers you start inside with --network=host', why: "this runtime has no firewall for Docker's own network: a container on it cannot reach the network, though -p still serves a port" },
];

const gvisor: RuntimeClass = {
  name: 'runsc', title: 'gVisor', boundary: 'user_space_kernel', capabilities: {}, unavailable: { kvm: 'a kernel in user space has no hardware virtualisation to give' },
  live_changes: { gateway_given: [], resources: false, mounts: false, capabilities: [] }, network_egress: ['open'], recreate_preserves_files: false, keeps_software_on_stop: false, memory_overhead_mb: 64, limits,
};
const runc: RuntimeClass = { ...gvisor, name: 'runc', title: 'runc', boundary: 'shared_kernel', unavailable: {}, limits: [] };

const template = (spec: Record<string, unknown>) => ({ id: 't', driver: 'docker', spec }) as unknown as Template;
const host = (...runtimes: RuntimeClass[]) => ({ drivers: [{ name: 'docker', runtime_classes: runtimes.map((entry) => entry.name), runtimes }] }) as never;
afterEach(async () => { await setLocale('en'); });

describe('the line under the runtime choice', () => {
  it('names what the runtime shares and gives the first limit in one line, with the rest behind a "?"', async () => {
    show(<RuntimeLimitsLine runtime={gvisor} limits={limitsTouching(limits, templateSubjects({}))} />);
    const line = screen.getByTestId('runtime-limits');
    expect(line).toHaveTextContent('gVisor: its own kernel written in user space');
    expect(line).toHaveTextContent('A mount or device change makes the environment again (this runtime cannot change them while it runs; software installed outside the volumes is removed)');
    expect(line).not.toHaveTextContent('--network=host');
    await userEvent.click(within(line).getByRole('button', { name: 'Help: Limits here' }));
    const card = await screen.findByRole('dialog');
    expect(card).toHaveTextContent('A mount or device change makes the environment again');
    expect(card).toHaveTextContent('a kernel in user space has no hardware virtualisation to give');
  });

  it('tells the port limit only where Docker inside is held or offered', async () => {
    const without = limitsTouching(limits, templateSubjects({ base_capabilities: ['ptrace'], allowed_capabilities: ['kvm'] }));
    expect(without.map((entry) => entry.key)).toEqual(['live_changes_lost']);
    const offered = limitsTouching(limits, templateSubjects({ allowed_capabilities: ['docker_inside'] }));
    expect(offered.map((entry) => entry.key)).toEqual(['live_changes_lost', 'docker_inside_network']);
    const { unmount } = show(<RuntimeLimitsLine runtime={gvisor} limits={offered} />);
    await userEvent.click(screen.getByRole('button', { name: 'Help: Limits here' }));
    expect(await screen.findByRole('dialog')).toHaveTextContent('Run the containers you start inside with --network=host');
    unmount();
  });
});

describe('the runtime choice of the dialog that makes an environment', () => {
  it('uses the reported host default even when another class is listed first', () => {
    const cluster = { drivers: [{ name: 'kubernetes', title: 'Kubernetes', default_runtime: 'runc', runtime_classes: ['runsc', 'runc'], runtimes: [gvisor, runc] }] } as never;
    const recipe = { driver: 'kubernetes', spec: {} } as unknown as Template;
    show(<RuntimeChoice template={recipe} hosts={[cluster]} value="" onChange={() => undefined} />);
    expect(screen.getByRole('combobox', { name: 'Runtime' })).toHaveTextContent('runc');
    expect(screen.getByText('Kubernetes · runc')).toHaveClass('sr-only');
  });

  it('does not invent a default where candidate hosts disagree and pins an explicit choice', async () => {
    const first = { drivers: [{ name: 'kubernetes', title: 'Kubernetes', default_runtime: 'runsc', runtime_classes: ['runsc', 'runc'], runtimes: [gvisor, runc] }] } as never;
    const second = { drivers: [{ name: 'kubernetes', title: 'Kubernetes', default_runtime: 'runc', runtime_classes: ['runc', 'runsc'], runtimes: [runc, gvisor] }] } as never;
    const recipe = { driver: 'kubernetes', spec: {} } as unknown as Template;
    const changed = vi.fn();
    show(<RuntimeChoice template={recipe} hosts={[first, second]} value="" onChange={changed} />);
    const select = screen.getByRole('combobox', { name: 'Runtime' });
    expect(select).toHaveTextContent('Use default runtime');
    expect(screen.getByText('Kubernetes')).toHaveClass('sr-only');
    await userEvent.click(select);
    await userEvent.click(screen.getByRole('option', { name: /gVisor/ }));
    expect(changed).toHaveBeenLastCalledWith('runsc');
  });

  it('uses the reported driver title and tells the mount limit before the first mount', () => {
    const limit = { key: 'kubernetes_mount_restart', touches: ['mounts'], todo: null, why: 'To change a mount, the environment restarts.' };
    const kubernetesClass = { ...runc, limits: [limit] };
    const cluster = { drivers: [{ name: 'kubernetes', title: 'Kubernetes', runtime_classes: ['runc', 'runsc'], runtimes: [kubernetesClass, gvisor] }] } as never;
    const recipe = { driver: 'kubernetes', spec: { runtime: 'runc' } } as unknown as Template;
    show(<RuntimeChoice template={recipe} hosts={[cluster]} value="" onChange={() => undefined} />);
    expect(screen.getByText('Kubernetes · runc')).toHaveClass('sr-only');
    expect(screen.getByTestId('runtime-limits')).toHaveTextContent('To change a mount, the environment restarts.');
  });

  it('is a line and no choice where the host offers one runtime', () => {
    show(<RuntimeChoice template={template({ runtime: 'runsc' })} hosts={[host(gvisor)]} value="" onChange={() => undefined} />);
    expect(screen.queryByRole('combobox')).toBeNull();
    expect(screen.getByTestId('runtime-limits')).toHaveTextContent('gVisor: its own kernel written in user space');
  });

  it('is a labelled choice with what each runtime shares where the host offers several, and the template\'s own is the one chosen', () => {
    show(<RuntimeChoice template={template({ runtime: 'runsc', allowed_runtimes: ['runsc', 'runc'] })} hosts={[host(runc, gvisor)]} value="" onChange={() => undefined} />);
    expect(screen.getByRole('combobox', { name: 'Runtime' })).toHaveTextContent('gVisor');
    expect(screen.getByTestId('runtime-limits')).toHaveTextContent('gVisor: its own kernel written in user space');
  });

  it('asks outright for the first runtime the place allows where the template\'s own is one it refuses', async () => {
    const asked: string[] = [];
    show(<RuntimeChoice template={template({ runtime: 'runc' })} hosts={[host(runc, gvisor)]} value="" onChange={(next) => asked.push(next)} policy={{ '*': 'allow', runc: 'forbid' }} />);
    await waitFor(() => expect(asked).toEqual(['runsc']));
  });

  it('leaves the template\'s own to the server where the place has no say', () => {
    const asked: string[] = [];
    show(<RuntimeChoice template={template({ runtime: 'runc' })} hosts={[host(runc, gvisor)]} value="" onChange={(next) => asked.push(next)} policy={{ '*': 'allow' }} />);
    expect(asked).toEqual([]);
  });

  it('keeps refused runtimes visible with their reason when the place requires the only permitted one', async () => {
    show(<RuntimeChoice template={template({ runtime: 'runsc' })} hosts={[host(runc, gvisor)]} value="" onChange={() => undefined} policy={{ runsc: 'require' }} />);
    await userEvent.click(screen.getByRole('combobox', { name: 'Runtime' }));
    const refused = await screen.findByRole('option', { name: /runc/ });
    expect(refused).toHaveAttribute('aria-disabled', 'true');
    expect(refused).toHaveTextContent(i18n.t('isolation.select.required_elsewhere'));
  });

  it('shows the reason when the only host runtime is forbidden', async () => {
    show(<RuntimeChoice template={template({ runtime: 'runsc' })} hosts={[host(gvisor)]} value="" onChange={() => undefined} policy={{ runsc: 'forbid' }} />);
    await userEvent.click(screen.getByRole('combobox', { name: 'Runtime' }));
    const refused = await screen.findByRole('option', { name: /gVisor/ });
    expect(refused).toHaveAttribute('aria-disabled', 'true');
    expect(refused).toHaveTextContent(i18n.t('isolation.select.forbidden'));
  });

  it('keeps the chooser when an explicit runtime is now forbidden, so an allowed runtime can be chosen', async () => {
    const changed = vi.fn();
    show(<RuntimeChoice template={template({ runtime: 'runsc' })} hosts={[host(gvisor, runc)]} value="runsc" onChange={changed} policy={{ runsc: 'forbid' }} />);
    await userEvent.click(screen.getByRole('combobox', { name: 'Runtime' }));
    expect(await screen.findByRole('option', { name: /gVisor/ })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('option', { name: /^runc/ }));
    expect(changed).toHaveBeenLastCalledWith('runc');
  });

  it('says nothing where no host reports a runtime', () => {
    const { container } = render(<RuntimeChoice template={template({})} hosts={[{ drivers: [{ name: 'docker', runtime_classes: [] }] } as never]} value="" onChange={() => undefined} />);
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole('combobox')).toBeNull();
    expect(screen.queryByTestId('runtime-limits')).toBeNull();
  });
});

describe('the Runtime and Limits rows of an environment', () => {
  const isolation = { driver: 'docker', driver_title: 'Docker', runtime: 'runsc', runtime_title: 'gVisor', boundary: 'user_space_kernel', limits, unavailable: { kvm: 'a kernel in user space has no hardware virtualisation to give' } } as unknown as RuntimeIsolation;

  it('are the pair, its hint, and every limit as what to do and why', () => {
    const rows = environmentRuntimeRows(isolation, i18n.t.bind(i18n));
    expect(rows.map((row) => row.name)).toEqual(['Runtime', 'Limits here']);
    show(<dl>{rows.map((row) => <div key={row.name}><dt>{row.name}</dt><dd>{row.value}</dd></div>)}</dl>);
    expect(screen.getByText('Docker · gVisor')).toBeVisible();
    expect(screen.getByText('gVisor: its own kernel written in user space')).toBeVisible();
    expect(screen.getByText(/^Run the containers you start inside with --network=host \(/)).toBeVisible();
  });

  it('are named in the head of the page, with what the runtime shares as its hint', () => {
    expect(namesRuntime(isolation)).toBe(true);
    expect(namesRuntime(null)).toBe(false);
    expect(namesRuntime({ ...isolation, runtime_title: null })).toBe(false);
    if (!namesRuntime(isolation)) throw new Error('the view names its runtime');
    show(<RuntimePair isolation={isolation} />);
    expect(screen.getByText('Docker · gVisor')).toHaveAttribute('title', 'gVisor: its own kernel written in user space');
  });

  it('stay absent for an unreported environment and describe a machine without inventing a runtime', () => {
    expect(environmentRuntimeRows(null, i18n.t.bind(i18n))).toEqual([]);
    const machine: RuntimeIsolation = { ...isolation, driver: 'host', driver_title: 'Registered directories', runtime: null, runtime_title: null, boundary: 'none', limits: [], unavailable: {} };
    const rows = environmentRuntimeRows(machine, i18n.t.bind(i18n));
    expect(rows.map((row) => row.name)).toEqual(['Runtime', 'Limits here']);
    show(<dl>{rows.map((row) => <div key={row.name}><dt>{row.name}</dt><dd>{row.value}</dd></div>)}</dl>);
    expect(screen.getByText('Registered directories')).toBeVisible();
    expect(screen.getByText(i18n.t('isolation.boundary.none'))).toBeVisible();
  });

  it('uses the ordinary Docker inside label for a refused capability instead of its protocol key', () => {
    const rows = environmentRuntimeRows({ ...isolation, unavailable: { docker_inside: 'Docker inside needs a runtime with a kernel of its own' } }, i18n.t.bind(i18n));
    const view = show(<>{rows.map((row) => <div key={row.name}>{row.value}</div>)}</>);
    expect(view.container).toHaveTextContent('Docker inside:');
    expect(view.container).not.toHaveTextContent('docker_inside:');
  });

  it('translates shipped driver names, Runtime and Limits rows into Chinese while keeping runtime names', async () => {
    await setLocale('zh-CN');
    const rows = environmentRuntimeRows({ ...isolation, driver_title: 'Docker containers', runtime: 'runc', runtime_title: 'runc', boundary: 'shared_kernel', unavailable: {}, limits: [] }, i18n.t.bind(i18n));
    expect(rows.map((row) => row.name)).toEqual(['运行时', '此处的限制']);
    show(<dl>{rows.map((row) => <div key={row.name}><dt>{row.name}</dt><dd>{row.value}</dd></div>)}</dl>);
    expect(screen.getByText('Docker 容器 · runc')).toBeVisible();
    expect(screen.getByText(i18n.t('isolation.hint', { runtime: 'runc', boundary: i18n.t('isolation.boundary.shared_kernel') }))).toBeVisible();
  });

  it('names the Docker driver in Chinese without translating the runtime name', async () => {
    await setLocale('zh-CN');
    show(<RuntimePair isolation={isolation} />);
    expect(screen.getByText('Docker 驱动 · gVisor')).toBeVisible();
  });
});
