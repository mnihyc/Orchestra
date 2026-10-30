/** A report goes directly to a file chosen by its reader, with no full report held by the page. */
import type { ImportReportFormat } from '@/generated/api-types';

export interface ImportReportFile {
  write: (bytes: Uint8Array<ArrayBuffer>) => Promise<void>;
  close: () => Promise<void>;
  abort: () => Promise<void>;
}

interface ReportPicker {
  showSaveFilePicker?: (options: { suggestedName: string; types: { description: string; accept: Record<string, string[]> }[] }) => Promise<{ createWritable: () => Promise<ImportReportFile> }>;
}

export function canSaveImportReport(): boolean {
  return typeof (window as ReportPicker).showSaveFilePicker === 'function';
}

/** Ask during the person's action, before making an authenticated request. Cancellation reads nothing. */
export async function chooseImportReportFile(format: ImportReportFormat): Promise<ImportReportFile | null> {
  const pick = (window as ReportPicker).showSaveFilePicker;
  if (pick === undefined) throw new Error('Direct file saving unavailable');
  try {
    const handle = await pick.call(window, { suggestedName: `import-report.${format}`, types: [{ description: format.toUpperCase(), accept: { [format === 'csv' ? 'text/csv' : 'application/json']: [`.${format}`] } }] });
    return await handle.createWritable();
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') return null;
    throw error;
  }
}
