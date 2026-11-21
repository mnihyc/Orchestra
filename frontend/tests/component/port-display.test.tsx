import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { show } from './support';
import * as displays from '@/api/displays';
import { useBrowserRequests } from '@/features/browser/store';
import { PortDisplayAction, portDisplayOpener } from '@/features/ports/PortDisplayAction';
import { displayGreeting } from '@/features/ports/displayGreeting';

it.each(['', 'test password'])('opens the verified port through the display door with password %j', async (password) => {
  const user = userEvent.setup();
  const returned = { id: 'adopted-vm' } as Awaited<ReturnType<typeof displays.openDisplay>>;
  const opened = vi.spyOn(displays, 'openDisplay').mockResolvedValue(returned);
  const reveal = vi.fn();
  const attach = portDisplayOpener('work', reveal);
  const probe = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  show(<PortDisplayAction environment="lab" port={5901} attach={attach} probe={probe} />);
  expect(probe).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: 'Open as display' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Check for display' }));
  await user.click(screen.getByRole('button', { name: 'No VNC greeting' }));
  if (password) await user.type(screen.getByLabelText(/password/i), password);
  await user.click(screen.getByRole('button', { name: 'Open as display' }));
  expect(opened).toHaveBeenCalledWith('lab', { attach: { port: 5901, ...(password ? { password } : {}) } }, 'work');
  expect(useBrowserRequests.getState().display).toBe(returned.id);
  expect(reveal).toHaveBeenCalledOnce();
  opened.mockRestore();
  useBrowserRequests.getState().takeDisplay();
});

it('recognizes fragmented version greetings only at the start of the stream', () => {
  const bytes = new TextEncoder();
  const check = displayGreeting();
  expect(check(bytes.encode('RFB 003.'))).toBeUndefined();
  expect(check(bytes.encode('008\nmore data'))).toBe(true);
  expect(displayGreeting()(bytes.encode('HTTP RFB 003.008\n'))).toBe(false);
  expect(displayGreeting()(bytes.encode('RFB 003.999\n'))).toBe(false);
});
