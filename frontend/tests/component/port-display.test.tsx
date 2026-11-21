import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { show } from './support';
import { PortDisplayAction } from '@/features/ports/PortDisplayAction';
import { displayGreeting } from '@/features/ports/displayGreeting';

it('offers the display door only after the admitted listener sends a complete RFB greeting', async () => {
  const user = userEvent.setup();
  const attach = vi.fn();
  const probe = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  show(<PortDisplayAction environment="lab" port={5901} attach={attach} probe={probe} />);
  expect(probe).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: 'Open as display' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Check for display' }));
  await user.click(screen.getByRole('button', { name: 'No VNC greeting' }));
  await user.click(screen.getByRole('button', { name: 'Open as display' }));
  expect(attach).toHaveBeenCalledWith('lab', 5901);
});

it('recognizes fragmented version greetings only at the start of the stream', () => {
  const bytes = new TextEncoder();
  const check = displayGreeting();
  expect(check(bytes.encode('RFB 003.'))).toBeUndefined();
  expect(check(bytes.encode('008\nmore data'))).toBe(true);
  expect(displayGreeting()(bytes.encode('HTTP RFB 003.008\n'))).toBe(false);
  expect(displayGreeting()(bytes.encode('RFB 003.999\n'))).toBe(false);
});
