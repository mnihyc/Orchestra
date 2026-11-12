/** Finder keeps the row target of the permission page it covers, only for that opening. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Activity, useEffect, useState } from 'react';
import { expect, it, vi } from 'vitest';
import { registerRowOpener, useRowOpener } from '@/features/finder/pageRows';
import { show } from './support';

it('retains a covered page target and retires it before another finder opening after the page closes', async () => {
  const open = vi.fn(), user = userEvent.setup();
  function PermissionPage() {
    useEffect(() => registerRowOpener(open, ['topic.archive']), []);
    return <p>Permission page</p>;
  }
  function Finder({ active, close }: { active: boolean; close: () => void }) {
    const opener = useRowOpener(active);
    return <>
      <p role="status">{opener === null ? 'No page rows' : 'Page rows ready'}</p>
      {active && opener?.permissions?.has('topic.archive') === true && <button onClick={() => { opener('topic.archive'); close(); }}>Archive row</button>}
    </>;
  }
  function App() {
    const [active, setActive] = useState(false), [page, setPage] = useState(true);
    return <>
      <button onClick={() => setActive(true)}>Find</button>
      <button onClick={() => setPage(false)}>Close page</button>
      <Activity mode={active ? 'hidden' : 'visible'}>{page && <PermissionPage />}</Activity>
      <Finder active={active} close={() => setActive(false)} />
    </>;
  }
  show(<App />);
  expect(await screen.findByText('Page rows ready')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Find' }));
  expect(screen.getByText('Permission page')).not.toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Archive row' }));
  expect(open).toHaveBeenCalledExactlyOnceWith('topic.archive');
  expect(screen.getByText('Permission page')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Close page' }));
  expect(screen.getByRole('status')).toHaveTextContent('No page rows');
  await user.click(screen.getByRole('button', { name: 'Find' }));
  expect(screen.queryByRole('button', { name: 'Archive row' })).not.toBeInTheDocument();
});
