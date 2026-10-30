/** A topic repository is one optional choice, a plain directory says nothing of Git, and a machine refusing writes explains its policy while the first input stays held. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { SourcePreparation } from '@/features/source/Preparation';
import { RepositoryChoice } from '@/features/source/RepositoryChoice';
import type { Thread } from '@/generated/api-types';
import { show } from './support';

vi.mock('@/api/source', () => ({ prepareSource: vi.fn() }));

it('offers the topic repository as one compact optional choice with a working opt-out', async () => {
  const onChange = vi.fn();
  function Form() {
    const [checked, setChecked] = useState(true);
    return <RepositoryChoice source={{ repository_url: 'https://example.test/team/repo.git', branch: null, secret_name: null, from: null }} checked={checked} onChange={(next) => { setChecked(next); onChange(next); }} />;
  }
  const { container } = show(<Form />);
  const choice = screen.getByRole('checkbox', { name: "Clone the topic's repository into a new directory" });
  expect(choice).toBeChecked();
  expect(screen.getByText('https://example.test/team/repo.git')).toBeVisible();
  // The label says what it does; no paragraph of guidance stands beside it. A machine's refusal is said on the thread.
  expect(container.querySelector('[data-choice-descriptions]')).toBeNull();
  expect(choice).not.toHaveAttribute('aria-describedby');
  expect(container).not.toHaveTextContent(/machine policy|remote writes/);
  await userEvent.click(choice);
  expect(choice).not.toBeChecked();
  expect(onChange).toHaveBeenLastCalledWith(false);
  await userEvent.click(choice);
  expect(choice).toBeChecked();
  expect(onChange).toHaveBeenLastCalledWith(true);
});

it('offers nothing when the topic names no repository', () => {
  const { container } = show(<RepositoryChoice source={{ repository_url: null, branch: null, secret_name: null, from: null }} checked onChange={vi.fn()} />);
  expect(screen.queryByRole('checkbox')).toBeNull();
  expect(container).not.toHaveTextContent(/Git|repository/i);
});

it('names the write policy and recovery without exposing the protocol reason', () => {
  const thread = {
    id: 'source-policy', environment: { id: 'environment-policy', name: 'Workspace' }, environment_permissions: ['environment.manage'],
    source: { kind: 'repository', directory: 'web', repository_url: 'https://example.test/team/repo.git', branch: 'main',
      head: null, state: 'failed', job: null, reason: 'workspace.remote_write_disabled' },
  } as unknown as Thread;
  const { container } = show(<SourcePreparation thread={thread} />);
  expect(screen.getByText(/managers can enable remote writes in the environment and machine policy/)).toBeVisible();
  expect(screen.getByText('The first message waits until the source is ready.')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Prepare again' })).toBeEnabled();
  expect(container).not.toHaveTextContent('workspace.remote_write_disabled');
});

it('waits for a plain directory without any word of Git or a repository', () => {
  const thread = {
    id: 'source-plain', state: 'starting', blocked: null, environment: { id: 'environment-plain', name: 'Workspace' }, environment_permissions: ['environment.manage'],
    source: { kind: 'directory', directory: 'notes', repository_url: null, branch: null, head: null, state: 'pending', job: null, reason: null },
  } as unknown as Thread;
  const { container } = show(<SourcePreparation thread={thread} />);
  expect(container.querySelector('[data-source-state="pending"]')).toBeInTheDocument();
  expect(screen.getByText('Using existing files in notes.')).toBeVisible();
  expect(container).not.toHaveTextContent(/Git|repository/i);
});
