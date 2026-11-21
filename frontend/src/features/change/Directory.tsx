/** A thread chooses its short workspace directory through the shared native folder control. */
import { FolderOpen } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { Door } from '@/api/workspace';
import { DirectoryControl } from '@/features/directory/DirectoryControl';
import { Button } from '@/ui/Button';

export function ChangeDirectory({ value, current, door, onChange }: {
  value: string | null; current: string; door: Door | null; onChange: (value: string | null) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  if (!editing && value === null) return <Button size="sm" variant="ghost" icon={<FolderOpen aria-hidden />} onClick={() => setEditing(true)}>{t('change.directory.change')}</Button>;
  return <div className="flex min-w-0 flex-wrap items-end gap-2" data-change-directory>
    <DirectoryControl target={{ kind: 'environment', door }} value={value ?? current} onChange={onChange} />
    <Button size="sm" variant="ghost" onClick={() => { onChange(null); setEditing(false); }}>{t('change.directory.keep')}</Button>
  </div>;
}
