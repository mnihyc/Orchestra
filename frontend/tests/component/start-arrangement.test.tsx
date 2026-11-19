/** The arrangement step of the dialog that starts a thread: the one the server would take is marked and ticked, a way that cannot be had is off with the server's own sentence, and what is ticked is what the dialog is told, the first row as much as another. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { describeIsolation } from '@/features/start/words';
import { ArrangementSection } from '@/features/start/Arrangement';
import { NO_PICKS, resolve } from '@/features/start/choices';
import { i18n, setLocale } from '@/i18n';

import { agent, arrangement, options, provider } from '../unit/start-fixtures';
import { show } from './support';

const SUGGEST = { suggest: true };

function draw(rows = [arrangement('key_separated', 1), arrangement('separated', 2), arrangement('credential_shared', 3, false, 'arrangement_not_allowed')], onChange: (type: string) => void = () => undefined) {
  const answer = options({ providers: [provider('p1', { agents: [agent('a1', { arrangements: rows })] })] });
  return show(<ArrangementSection resolved={resolve(answer, NO_PICKS, SUGGEST)} refreshing={false} onChange={onChange} />);
}

describe('the ways an agent can be placed', () => {
  it.each(['en', 'zh-CN'] as const)('names a declared third-program tool set in %s', async (locale) => {
    await setLocale(locale);
    try {
      const named = { executor_ref: { kind: 'research-exec', title: { en: 'Research executor', 'zh-CN': '研究执行器' }, tool_set: { name: 'research', title: { en: 'Research tools', 'zh-CN': '研究工具' }, summary: { en: 'Tools for research', 'zh-CN': '研究工具集' } } } };
      const isolation = { ...arrangement('separated', 1).isolation, ...named };
      const words = describeIsolation(isolation, i18n.t);
      expect(words).toContain(locale === 'en' ? 'Research tools' : '研究工具');
      expect(words).not.toContain('research-exec');
    } finally { await setLocale('en'); }
  });

  it('are listed in server order, with its default visibly marked and its explanation available', () => {
    draw();
    const rows = screen.getAllByRole('radio');
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveAccessibleName('Forward model calls');
    expect(rows[1]).toHaveAccessibleName('Run with provider');
    expect(rows[2]).toHaveAccessibleName('Copy sign-in access');
    expect(rows[0]).toBeChecked();
    expect(screen.getByText('Default')).toBeVisible();
    const first = rows[0];
    if (first === undefined) throw new Error('The first ranked arrangement is missing');
    expect(first).toHaveAccessibleDescription(expect.stringContaining('Default'));
    expect(document.querySelector('[data-choice-selected-description]')).toBeNull();
    expect(screen.getByRole('radio', { name: 'Forward model calls' })).toBeChecked();
  });

  it('tell the dialog which is ticked when another row is chosen', async () => {
    const chosen = vi.fn();
    draw(undefined, chosen);
    await userEvent.click(screen.getByRole('radio', { name: /Run with provider/ }));
    expect(chosen).toHaveBeenCalledWith('separated');
  });

  it('that cannot be had are off with the server’s sentence, and cannot be chosen', async () => {
    const chosen = vi.fn();
    const denied = { ...arrangement('credential_shared', 3, false, 'arrangement_not_allowed'), detail: 'This provider does not allow a copy of its sign-in.' };
    draw([arrangement('key_separated', 1), arrangement('separated', 2), denied], chosen);
    const closed = screen.getByRole('radio', { name: /Copy sign-in access/ });
    expect(closed).toBeDisabled();
    expect(closed).toHaveAccessibleDescription(`${describeIsolation(denied.isolation, i18n.t)} ${denied.detail}`);
    expect(closed).toHaveAttribute('title', denied.detail);
    expect(screen.queryByText('arrangement_not_allowed detail')).not.toBeInTheDocument();
    await userEvent.click(closed);
    expect(chosen).not.toHaveBeenCalled();
  });

  it('wait for a runnable profile where none can be had, and the first reason is said', () => {
    const chosen = vi.fn();
    draw([arrangement('separated', 1, false, 'executor_missing'), arrangement('key_separated', 2, false, 'arrangement_not_allowed')], chosen);
    expect(screen.queryByRole('radiogroup')).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('start.reason.executor_missing'));
    expect(screen.getByRole('alert')).not.toHaveTextContent('executor_missing');
    expect(chosen).not.toHaveBeenCalled();
  });

  it('are not offered while the harness of the profile has not reported what it can do', () => {
    const answer = options({ providers: [provider('p1', { agents: [agent('a1', { reason: 'harness_unreported' })] })] });
    show(<ArrangementSection resolved={resolve(answer, NO_PICKS, SUGGEST)} refreshing={false} onChange={() => undefined} />);
    expect(screen.queryByRole('radiogroup')).toBeNull();
    expect(screen.getByText(/The machine has not reported what this agent can do yet/)).toBeVisible();
  });
});
