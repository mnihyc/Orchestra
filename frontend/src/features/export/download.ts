/** Fetching an archive and saving it, with how far it has come kept where every screen that offers Download can show it.
 *
 * An archive can be large, and a transfer has no limit on its whole time: it is read in pieces and its progress
 * is in bytes. One archive is fetched once at a time; asking again while it comes does nothing. The same
 * archive can be fetched again later, until it is removed.
 */
import { create } from 'zustand';

import { downloadExport, exportKeys } from '@/api/exports';
import { ApiError } from '@/api/client';
import { queryClient } from '@/api/query';
import { i18n } from '@/i18n';
import { toast } from '@/ui/Toast';

import { saveFile } from './save';

export interface Downloading {
  received: number;
  total: number | null;
}

interface DownloadStore {
  /** The archives being fetched, by identifier. */
  active: ReadonlyMap<string, Downloading>;
}

export const useDownloads = create<DownloadStore>(() => ({ active: new Map() }));

function setProgress(id: string, progress: Downloading | null): void {
  useDownloads.setState((state) => {
    const active = new Map(state.active);
    if (progress === null) active.delete(id);
    else active.set(id, progress);
    return { active };
  });
}

/** How far the fetching of an archive has come; `null` when it is not being fetched. */
export function useDownloading(id: string): Downloading | null {
  return useDownloads((state) => state.active.get(id) ?? null);
}

/** Fetches an archive and hands it to the browser to save. A failure is told in a notice, in the words of its code. */
export async function fetchArchive(id: string): Promise<void> {
  if (useDownloads.getState().active.has(id)) return;
  setProgress(id, { received: 0, total: null });
  try {
    const file = await downloadExport(id, (progress) => setProgress(id, progress));
    saveFile(file.blob, file.name);
  } catch (error) {
    const gone = error instanceof ApiError && error.is('gone');
    toast({
      tone: 'danger',
      title: i18n.t(gone ? 'export.download.gone' : 'export.download.failed'),
      ...(gone ? { description: i18n.t('export.download.goneAbout') } : {}),
    });
    // The list is read again: an archive that was removed meanwhile is not offered any more.
    if (gone) void queryClient.invalidateQueries({ queryKey: exportKeys.all });
  } finally {
    setProgress(id, null);
    // The first fetch is told to the list: it says when the archive was first fetched.
    void queryClient.invalidateQueries({ queryKey: exportKeys.mine });
  }
}
