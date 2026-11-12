/** A form opened from a button's menu gives the keyboard back to that button, also after another form covered it meanwhile. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';

import { Dialog } from '@/ui/Dialog';
import { Menu } from '@/ui/Menu';
import { show } from './support';

function Screen({ findAtOnce = false }: { findAtOnce?: boolean }) {
  const [access, setAccess] = useState(false);
  const [finder, setFinder] = useState(false);
  // A second form can open before the menu has finished giving up the keyboard.
  const share = () => { setAccess(true); if (findAtOnce) setTimeout(() => setFinder(true), 0); };
  return <>
    <Menu label="Actions" trigger={<button>Actions</button>} entries={[{ id: 'share', label: 'Share', onSelect: share }]} />
    <button>Elsewhere</button>
    <Dialog title="Access" open={access} onOpenChange={setAccess}><button onClick={() => setFinder(true)}>Find rows</button></Dialog>
    <Dialog title="Find" open={finder} onOpenChange={setFinder}><button onClick={() => setFinder(false)}>Archive row</button></Dialog>
  </>;
}

async function openFromMenu(user: ReturnType<typeof userEvent.setup>) {
  const trigger = screen.getByRole('button', { name: 'Actions' });
  await user.click(trigger);
  await user.click(screen.getByRole('menuitem', { name: 'Share' }));
  expect(await screen.findByRole('dialog', { name: 'Access' })).toBeVisible();
  return trigger;
}

describe('the keyboard returns to the button whose menu opened a form', () => {
  it('after the form is closed', async () => {
    const user = userEvent.setup();
    show(<Screen />);
    const trigger = await openFromMenu(user);
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Access' })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('after another form covered it and went away', async () => {
    const user = userEvent.setup();
    show(<Screen />);
    const trigger = await openFromMenu(user);
    await user.click(await screen.findByRole('button', { name: 'Find rows' }));
    await user.click(await screen.findByRole('button', { name: 'Archive row' }));
    expect(await screen.findByRole('dialog', { name: 'Access' })).toBeVisible();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Access' })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('also when a second form opens before the menu has finished', async () => {
    const user = userEvent.setup();
    show(<Screen findAtOnce />);
    const trigger = screen.getByRole('button', { name: 'Actions' });
    await user.click(trigger);
    await user.click(screen.getByRole('menuitem', { name: 'Share' }));
    await user.click(await screen.findByRole('button', { name: 'Archive row' }));
    expect(await screen.findByRole('dialog', { name: 'Access' })).toBeVisible();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Access' })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('but a form opened from another control keeps that control', async () => {
    const user = userEvent.setup();
    function Other() {
      const [open, setOpen] = useState(false);
      return <>
        <Menu label="Actions" trigger={<button>Actions</button>} entries={[{ id: 'noop', label: 'Nothing', onSelect: () => undefined }]} />
        <button onClick={() => setOpen(true)}>Open</button>
        <Dialog title="Plain" open={open} onOpenChange={setOpen}><span>Body</span></Dialog>
      </>;
    }
    show(<Other />);
    await user.click(screen.getByRole('button', { name: 'Actions' }));
    await user.click(screen.getByRole('menuitem', { name: 'Nothing' }));
    const opener = screen.getByRole('button', { name: 'Open' });
    await user.click(opener);
    expect(await screen.findByRole('dialog', { name: 'Plain' })).toBeVisible();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(opener).toHaveFocus());
  });
});
