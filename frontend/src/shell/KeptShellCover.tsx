/** Private kept names are painted only after the application's client confirms their reader. */
import { useMemo } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { readKeptShell, readShellGeometry, useCover } from './keptShell';
import { ShellPicture } from './ShellPicture';

export function KeptShellCover() {
  const covering = useCover((state) => state.covering);
  const reader = useStore(client.session, (state) => state.status === 'signed_in' ? state.userId : null);
  const kept = useMemo(() => reader === null ? null : readKeptShell(reader), [reader]);
  const geometry = useMemo(() => readShellGeometry(), []);
  return covering ? <ShellPicture kept={kept} geometry={geometry} /> : null;
}
