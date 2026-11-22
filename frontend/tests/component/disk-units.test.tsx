/** One disk reads the same everywhere: its size and free space in the decimal gigabytes every storage report uses. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { MachineCapacityLine } from '@/features/resources/machines/MachineCapacityLine';
import { setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });

it('says a machine\'s data disk in the gigabytes its Storage tab and its hosts give, its memory in binary units', () => {
  // The disk of df -B1: 267,900,809,216 bytes. Its connector reports the storage as 267 GB.
  show(<MachineCapacityLine facts={{ cpus: 10, memory_bytes: 38.1 * 1024 ** 3, data_volume_bytes: 267_900_809_216 }} />);
  expect(screen.getByText('10 CPUs · 38.1 GiB RAM · 267.9 GB data disk')).toBeVisible();
});
