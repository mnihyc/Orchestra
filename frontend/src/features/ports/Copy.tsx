/** A named Copy act for addresses, commands and the secret held only by its new-link dialog. */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { writeToClipboard } from '@/ui/clipboard';
import { COPIED_SHOWN_MS } from '@/ui/CopyButton';

export function PortCopy({ value, label }: { value: string; label: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (copied === null) return;
    const timer = setTimeout(() => setCopied(null), COPIED_SHOWN_MS);
    return () => clearTimeout(timer);
  }, [copied]);
  const copy = async () => {
    if (busy) return;
    setBusy(true);
    setCopied(null);
    try { setCopied(await writeToClipboard(value)); } finally { setBusy(false); }
  };
  return <span className="flex flex-wrap items-center gap-2">
    <Button size="sm" action="copy" pending={busy} onClick={() => void copy()}>{label}</Button>
    <span role="status" className="text-xs text-text-muted">{copied === null ? '' : t(copied ? 'ports.copied' : 'ports.copyRefused')}</span>
  </span>;
}
