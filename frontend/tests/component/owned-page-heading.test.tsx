/** An owned caption removes only redundant chrome; opening checks, metadata and acts remain. */
import { screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { PageHeader, PageSectionContext, Section } from '@/shell/Page';
import { SettingsFrame } from '@/features/settings/Frame';
import { show } from './support';

const calls = vi.hoisted(() => ({ opening: vi.fn() }));
vi.mock('@/features/access/opening', async (original) => ({
  ...await original<object>(), useAdminOpening: calls.opening,
}));
beforeEach(() => { calls.opening.mockClear(); });

it('retains the native opening hook when its surrounding scope already owns an otherwise empty header', () => {
  const view = show(<PageHeader title="Current section" titleInContext opening={{ kind: 'environment', id: 'environment-one' }} />);
  expect(calls.opening).toHaveBeenCalledWith('environment', 'environment-one', false);
  expect(screen.queryByRole('heading')).not.toBeInTheDocument();
  expect(view.container.querySelector('header')).toBeNull();
  view.rerender(<PageHeader title="Current section" titleInContext lead="The current native state" actions={<button>Refresh current data</button>} />);
  expect(screen.getByText('The current native state')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Refresh current data' })).toBeVisible();
});

it('uses the real owning heading and retains a reused body’s lead exactly once', () => {
  show(<PageSectionContext value={{ title: 'Current section', id: 'owner-heading' }}>
    <h2 id="owner-heading">Current section</h2>
    <SettingsFrame title="Current section" lead="Current native value provenance" label="Sections" current="general" items={[]}>
      <Section id="current-values" title="Current section" lead="Current native value provenance" action={<button>Change current value</button>}>
        <p>All current values remain visible</p>
      </Section>
    </SettingsFrame>
  </PageSectionContext>);
  expect(screen.getAllByRole('heading', { name: 'Current section' })).toHaveLength(1);
  expect(screen.getAllByText('Current native value provenance')).toHaveLength(1);
  expect(screen.getByRole('region', { name: 'Current section' })).toHaveAttribute('aria-labelledby', 'owner-heading');
  expect(screen.getByRole('button', { name: 'Change current value' })).toBeVisible();
  expect(screen.getByText('All current values remain visible')).toBeVisible();
});
