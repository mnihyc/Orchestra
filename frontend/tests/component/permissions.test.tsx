/** What a person who may not do something is shown: the act left out, switched off with its reason and still within reach, or one quiet line; and what a refusal says is missing, and whom to ask. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type { Problem, UserRef } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { PermissionGate } from '@/ui/PermissionGate';

import { show } from './support';

const REASON = 'You can’t manage Web. Ask Ada Lovelace, who owns it.';

function person(name: string, n: number): UserRef {
  return {
    id: `00000000-0000-4000-8000-${n.toString().padStart(12, '0')}`,
    kind: 'human',
    username: name.split(' ')[0]?.toLowerCase() ?? name,
    display_name: name,
    avatar: { text: name.slice(0, 1), color: n },
  };
}

function refusal(entries: object[]): ApiError {
  const problem: Problem = {
    type: 'orch:error:authz.denied',
    code: 'authz.denied',
    title: 'Denied',
    status: 403,
    detail: 'you cannot do this',
    request_id: 'req-1',
    required: 'project.interact',
    requestable: true,
    override: false,
    missing: entries,
  };
  return new ApiError(problem, null);
}

describe('an act that its reader may not do', () => {
  it('is left out where it would only be noise', () => {
    show(
      <PermissionGate allowed={false}>
        <Button>Rename…</Button>
      </PermissionGate>,
    );

    expect(screen.queryByRole('button', { name: 'Rename…' })).not.toBeInTheDocument();
  });

  it('is there for whoever may do it, and works', async () => {
    const user = userEvent.setup();
    const rename = vi.fn();
    show(
      <PermissionGate allowed>
        <Button onClick={rename}>Rename…</Button>
      </PermissionGate>,
    );

    await user.click(screen.getByRole('button', { name: 'Rename…' }));

    expect(rename).toHaveBeenCalledOnce();
  });

  it('stays switched off with what it takes, within reach of the keyboard, and does nothing when it is pressed', async () => {
    const user = userEvent.setup();
    const rename = vi.fn();
    show(
      <PermissionGate allowed={false} mode="block" reason={REASON}>
        <Button onClick={rename}>Rename…</Button>
      </PermissionGate>,
    );

    const button = screen.getByRole('button', { name: 'Rename…' });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    expect(button).toHaveAccessibleDescription(REASON);

    await user.tab();
    expect(button).toHaveFocus();
    await user.click(button);
    await user.keyboard('{Enter}');

    expect(rename).not.toHaveBeenCalled();
  });

  it('is replaced by one quiet line that says what it takes', () => {
    show(
      <PermissionGate allowed={false} mode="note" note={<p>{REASON}</p>}>
        <Button>New topic</Button>
      </PermissionGate>,
    );

    expect(screen.queryByRole('button', { name: 'New topic' })).not.toBeInTheDocument();
    expect(screen.getByText(REASON)).toBeInTheDocument();
  });
});

describe('a refusal that comes back from the server', () => {
  it('says what is missing, on what, and whom to ask, as the server lists it', async () => {
    show(
      <ErrorAlert
        error={refusal([
          {
            object: { id: '00000000-0000-4000-8002-000000000003', kind: 'project', name: 'Web', owner: person('Ada Lovelace', 1) },
            permissions: ['project.interact'],
            ask: [person('Ada Lovelace', 1), person('Grace Hopper', 2)],
            ask_more: 0,
            override: false,
            closed: null,
            options: null,
          },
        ])}
      />,
    );

    await userEvent.click(screen.getByRole('button', { name: 'Review access' }));
    const alert = screen.getByRole('dialog');
    expect(alert).toHaveTextContent('You can’t take part in Web.');
    expect(alert).not.toHaveTextContent('Interact Project');
    expect(alert).toHaveTextContent('Ask Ada Lovelace, Grace Hopper.');
  });

  it('counts the others who could give it without naming them, and says when nobody can', async () => {
    show(
      <ErrorAlert
        error={refusal([
          {
            object: { id: '00000000-0000-4000-8002-000000000003', kind: 'project', name: 'Web', owner: null },
            permissions: ['project.manage'],
            ask: [person('Ada Lovelace', 1)],
            ask_more: 3,
            override: false,
            closed: null,
            options: null,
          },
          {
            object: { id: '00000000-0000-4000-8002-000000000004', kind: 'org', name: 'Acme', owner: null },
            permissions: ['org.interact'],
            ask: [],
            ask_more: 0,
            override: false,
            closed: null,
            options: null,
          },
        ])}
      />,
    );

    await userEvent.click(screen.getByRole('button', { name: 'Review access' }));
    const alert = screen.getByRole('dialog');
    expect(alert).toHaveTextContent('You can’t manage Web.');
    expect(alert).toHaveTextContent('Ask Ada Lovelace, or one of 3 others.');
    expect(alert).toHaveTextContent('You can’t take part in Acme.');
    expect(alert).not.toHaveTextContent('Manage Project');
    expect(alert).not.toHaveTextContent('Interact Org');
    expect(alert).toHaveTextContent('Nobody can give this from here. Ask an administrator of the system.');
  });
});
