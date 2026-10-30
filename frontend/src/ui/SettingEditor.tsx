/** An in-row editor: opened on demand, then retained so closing it cannot discard unfinished input. */
import { Activity, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { Pencil } from 'lucide-react';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { SettingRow, type SettingRowProps } from '@/ui/SettingRow';

export interface SettingEditorProps extends Omit<SettingRowProps, 'value'> {
  /** The compact value, shown instead of the editor until it is opened. */
  summary: ReactNode;
  label: string;
  children: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  /** Some form cleanups discard validity. They retain effects and pause their data with the open state instead. */
  pauseEffects?: boolean;
  /** Registry controls edit in the value column; larger resource forms retain their details area. */
  placement?: 'inline' | 'details';
}

export function SettingEditor({ summary, label, children, open, onOpenChange, disabled = false, pauseEffects = true, placement = 'details', details, ...row }: SettingEditorProps) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const [localOpen, setLocalOpen] = useState(false);
  const expanded = open ?? localOpen;
  useLayoutEffect(() => {
    if (!expanded) return;
    const editor = content.current;
    const opener = trigger.current;
    if (placement === 'inline') {
      // A pencil begins editing here; keyboard input and Escape belong to the revealed value.
      // Larger details disclosures retain their existing focus behavior.
      const controls = Array.from(editor?.querySelectorAll<HTMLElement>('[data-autofocus], input:not([type="hidden"]), textarea, select, button, [contenteditable="true"]') ?? [])
        .filter((control) => (control.tabIndex >= 0 || control.hasAttribute('data-autofocus')) && !control.matches(':disabled,[aria-disabled="true"],[readonly]') && control.closest('[hidden],[inert],[aria-hidden="true"]') === null);
      const first = controls.find((control) => control.hasAttribute('data-autofocus')) ?? controls[0];
      first?.focus();
    }
    return () => {
      if (editor?.contains(document.activeElement) === true) opener?.focus();
    };
  }, [expanded, placement]);
  const [visited, setVisited] = useState(expanded);
  if (expanded && !visited) setVisited(true);
  const change = (next: boolean) => {
    if (disabled) return;
    setLocalOpen(next);
    onOpenChange?.(next);
  };
  const editor = <div ref={content} id={id} role="group" aria-label={label} hidden={!expanded} className="min-w-0 max-w-full"
    onKeyDown={(event) => {
      if (event.key !== 'Escape' || event.defaultPrevented || disabled) return;
      event.preventDefault();
      event.stopPropagation();
      change(false);
      trigger.current?.focus();
    }}>
    <Activity mode={expanded || !pauseEffects ? 'visible' : 'hidden'}>{(visited || expanded) && children}</Activity>
  </div>;
  return <SettingRow {...row}
    value={placement === 'inline' ? <div data-setting-inline-editor className="flex min-w-0 max-w-full flex-col gap-2">
      <div className="flex min-w-0 max-w-full items-start gap-2">
        <span id={`${id}-summary`} hidden={expanded} className="min-w-0 self-center">{summary}</span>
        <IconButton ref={trigger} data-setting-editor-trigger size="sm" label={label} icon={<Pencil aria-hidden />} aria-expanded={expanded} aria-controls={id}
          aria-describedby={`${id}-summary`} pending={disabled} onClick={() => change(!expanded)} />
      </div>
      {editor}
    </div> : <Button ref={trigger} data-setting-editor-trigger size="sm" aria-label={label} aria-expanded={expanded} aria-controls={id} aria-describedby={`${id}-summary`}
      pending={disabled} aria-busy={disabled || undefined} className="h-auto! min-h-(--control-sm) min-w-0 max-w-full py-1" onClick={() => change(!expanded)}>
      <span id={`${id}-summary`} className="truncate">{summary}</span>
    </Button>}
    detailsVisible={placement === 'details' ? expanded || details !== undefined : details !== undefined}
    details={placement === 'inline' ? details : <>{details}{editor}</>}
  />;
}
