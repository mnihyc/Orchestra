/** UA-32: the inspector reports composer choices once and shows only controls the harness declares. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ParametersPanel } from '@/features/inspector/Parameters';
import type { NodeSettingSetBody, ParameterView, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, memberAccess, userRef } from '../doubles/fixtures';
import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const server = vi.hoisted(() => ({
  parameters: null as ThreadParameters | null,
  rows: [] as ResolvedSetting[],
  read: vi.fn(),
  write: vi.fn<(id: string, key: string, body: NodeSettingSetBody) => Promise<ResolvedSetting>>(),
}));
vi.mock('@/api/settings', async (importOriginal) => ({
  ...await importOriginal<typeof SettingsApi>(),
  settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve(server.rows), staleTime: Infinity }),
}));
vi.mock('@/api/threads', async (importOriginal) => ({
  ...await importOriginal<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({
    queryKey: ['thread-parameters', id],
    queryFn: () => {
      server.read();
      return server.parameters === null ? Promise.reject(new Error('No parameter reply')) : Promise.resolve(server.parameters);
    },
    staleTime: Infinity,
  }),
  setThreadSetting: (id: string, key: string, body: NodeSettingSetBody) => server.write(id, key, body),
}));
vi.mock('@/realtime/live', () => ({ refreshThread: vi.fn() }));
// Approval-rule history is an adjacent, independent view; this suite exercises the real parameter body.
vi.mock('@/features/approvals/ThreadRules', () => ({ ThreadRules: () => null }));

const routingName = 'provider-internal/model-route';
const owner = userRef(me(1, 'ada', 'Ada'));
const thread = {
  id: 'thread-1', model: routingName, owner, archived: false, side_of: null,
  binding: { provider: { id: 'provider-1' } },
  access: { ...memberAccess, permissions: ['thread.view', 'thread.manage'] },
} as Thread;

function parameters(supported: string[] = []): ThreadParameters {
  return {
    model: { value: routingName, options: [{ ...MODEL_ESTIMATE, model_name: routingName, display_name: 'Mock Large', allowed: true, reason: null, parameters: { efforts: ['high'] } }], writable: true, source: { scope: 'thread', node: null } },
    effort: { value: 'high', writable: true, supported: false, source: { scope: 'default', node: null } },
    parameters: [],
    approval_mode: 'bypass', approval_modes: ['ask', 'bypass'], approval_reviewer: null,
    subagent_limits: { limits: { at_once: 8, depth: 1 }, supported },
  };
}

function modelChoices(): ParameterView[] {
  return [
    { key: 'service_tier', title: 'Speed', type: 'enum', options: { from_model: 'service_tiers' } },
    { key: 'program', title: 'Access program', type: 'enum', options: { from_model: 'access_programs' } },
    { key: 'context', title: 'Context', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } } },
  ].map((def): ParameterView => ({
    key: def.key, def, value: null, apply: 'next_turn',
    source: { scope: 'default', node: null }, locked_by: null, writable: true,
  }));
}

afterEach(async () => {
  useAppearance.getState().set({ theme: 'system', locale: 'en' });
  await setLocale('en');
});

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  describe(`Parameters in ${language}, ${theme}`, () => {
    const t = i18n.getFixedT(language);
    beforeEach(async () => {
      await setLocale(language);
      useAppearance.getState().set({ theme, locale: language });
      server.parameters = parameters();
      server.rows = [];
      server.read.mockClear();
      server.write.mockReset();
    });

    it('reports model and approval mode without a second picker and gives the change location once', async () => {
      show(<ParametersPanel access={{ thread }} />);
      const summary = await screen.findByRole('region', { name: t('inspector.parameters.currentChoices') });
      expect(within(summary).getAllByText(t('composer.model.exampleLarge'))).toHaveLength(1);
      expect(within(summary).getAllByText(t('composer.mode.labels.bypass'))).toHaveLength(1);
      expect(within(summary).getAllByText(t('inspector.parameters.changeInComposer'))).toHaveLength(1);
      expect(within(summary).queryByRole('combobox')).not.toBeInTheDocument();
      expect(within(summary).queryByRole('button')).not.toBeInTheDocument();
      expect(within(summary).queryByText(routingName)).not.toBeInTheDocument();
      await userEvent.click(within(summary).getByText(t('composer.model.exampleLarge')));
      await userEvent.click(within(summary).getByText(t('composer.mode.labels.bypass')));
      expect(server.write).not.toHaveBeenCalled();
      expect(server.read).toHaveBeenCalledTimes(1);
      expect(document.documentElement).toHaveAttribute('data-theme', theme);
    });

    it('uses one paragraph for no other parameters and omits every unsupported row', async () => {
      show(<ParametersPanel access={{ thread }} />);
      const line = await screen.findByText(t('inspector.parameters.noParameters'));
      expect(line.tagName).toBe('P');
      expect(line.closest('[data-testid="parameters-panel"]')).toBe(screen.getByTestId('parameters-panel'));
      expect(screen.getAllByText(t('inspector.parameters.noParameters'))).toHaveLength(1);
      expect(screen.queryByText(t('inspector.parameters.effort'))).not.toBeInTheDocument();
      expect(screen.queryByText(t('inspector.parameters.subagentsAtOnce', { count: 8 }))).not.toBeInTheDocument();
      expect(screen.queryByText(t('inspector.parameters.subagentDepth', { count: 1 }))).not.toBeInTheDocument();
      expect(screen.queryByText(t('inspector.parameters.subagentsApply'))).not.toBeInTheDocument();
      expect(screen.queryByText(t('inspector.parameters.subagentsUnsupported'))).not.toBeInTheDocument();
    });

    it.each([{ supported: ['at_once'] }, { supported: ['depth'] }, { supported: ['at_once', 'depth'] }])('keeps only the sub-agent controls that apply: $supported', async ({ supported }) => {
      server.parameters = parameters(supported);
      show(<ParametersPanel access={{ thread }} />);
      await screen.findByText(t('composer.model.exampleLarge'));
      for (const [key, words] of [
        ['at_once', t('inspector.parameters.subagentsAtOnce', { count: 8 })],
        ['depth', t('inspector.parameters.subagentDepth', { count: 1 })],
      ] as const) {
        if (supported.includes(key)) expect(screen.getByText(words)).toBeVisible();
        else expect(screen.queryByText(words)).not.toBeInTheDocument();
      }
      expect(screen.getAllByText(t('inspector.parameters.subagentsApply'))).toHaveLength(1);
      expect(screen.queryByText(t('inspector.parameters.subagentsUnsupported'))).not.toBeInTheDocument();
    });

    it('keeps a refused current model readable and has no alternate effort or model controls', async () => {
      const answer = parameters();
      answer.model.options[0] = { ...answer.model.options[0]!, allowed: false, reason: 'excluded_by_policy' };
      answer.effort.supported = true;
      server.parameters = answer;
      show(<ParametersPanel access={{ thread }} />);
      const summary = await screen.findByRole('region', { name: t('inspector.parameters.currentChoices') });
      expect(within(summary).getByText(t('composer.model.exampleLarge'))).toBeVisible();
      expect(within(summary).getByText(t('choiceCatalog.effort.high.name'))).toBeVisible();
      expect(within(summary).queryByRole('combobox')).not.toBeInTheDocument();
      expect(within(summary).queryByRole('button')).not.toBeInTheDocument();
      expect(server.write).not.toHaveBeenCalled();
    });

    it('omits model parameters for which this model offers nothing', async () => {
      const answer = parameters();
      answer.parameters = modelChoices();
      server.parameters = answer;
      show(<ParametersPanel access={{ thread }} />);
      await screen.findByText(t('composer.model.exampleLarge'));
      for (const key of ['composer.model.speed.title', 'composer.model.access.title', 'composer.model.context.title'] as const) {
        expect(screen.queryByText(t(key))).not.toBeInTheDocument();
      }
      expect(screen.getByText(t('inspector.parameters.noParameters'))).toBeVisible();
    });

    it('reports supported context, speed and access without another set of composer controls', async () => {
      const answer = parameters();
      const model = answer.model.options[0]!;
      model.parameters = {
        efforts: ['high'], service_tiers: [{ id: 'priority', label: 'Priority' }], default_service_tier: 'priority',
        access_programs: ['Team'], default_access_program: 'Team', context_max: 100_000, default_context: 100_000,
      };
      answer.parameters = modelChoices();
      server.parameters = answer;
      show(<ParametersPanel access={{ thread }} />);
      const summary = await screen.findByRole('region', { name: t('inspector.parameters.currentChoices') });
      await waitFor(() => expect(within(summary).getByText('100K')).toBeVisible());
      expect(within(summary).getByText('Priority')).toBeVisible();
      expect(within(summary).getByText('Team')).toBeVisible();
      expect(within(summary).queryByRole('combobox')).not.toBeInTheDocument();
      expect(within(summary).queryByRole('radio')).not.toBeInTheDocument();
      expect(within(summary).queryByRole('button')).not.toBeInTheDocument();
      expect(within(summary).getAllByText(t('inspector.parameters.changeInComposer'))).toHaveLength(1);
    });

    it('keeps eligible native rows and reports the manager once for a reader', async () => {
      const answer = parameters();
      answer.parameters = ['one', 'two'].map((key): ParameterView => ({
        key, def: { title: `Native ${key}`, type: 'bool', group: 'adapter-specific' }, value: false,
        apply: 'next_turn', source: { scope: 'default', node: null }, locked_by: null, writable: true,
      }));
      server.parameters = answer;
      const reader = { ...thread, access: { ...thread.access, permissions: ['thread.view'] } };
      show(<ParametersPanel access={{ thread: reader }} />);
      const one = await screen.findByRole('switch', { name: 'Native one' });
      const two = screen.getByRole('switch', { name: 'Native two' });
      await waitFor(() => expect(one).toBeDisabled());
      expect(two).toBeDisabled();
      expect(screen.getAllByText(t('inspector.parameters.needsManage', { name: 'Ada' }))).toHaveLength(1);
      expect(screen.queryByText(t('inspector.parameters.noParameters'))).not.toBeInTheDocument();
      await userEvent.click(one);
      expect(server.write).not.toHaveBeenCalled();
    });
  });
}
