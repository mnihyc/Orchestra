/** The finder and tab search share a prompt, its opening focus and its return destination. */
import { Dialog as Primitive } from 'radix-ui';
import type { ComponentProps, ReactNode } from 'react';

import { ContentBoundary } from '@/ui/ContentBoundary';
import { overlayClasses, useOpener } from '@/ui/Dialog';
import { PromptScope, usePrompt } from '@/ui/PromptScope';

export interface PaletteDialogProps {
  label: string;
  showTitle?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCloseAutoFocus?: ComponentProps<typeof Primitive.Content>['onCloseAutoFocus'];
  children: ReactNode;
}

export function PaletteDialog({ label, showTitle = false, open, onOpenChange, onCloseAutoFocus, children }: PaletteDialogProps) {
  const layer = usePrompt(open);
  const opener = useOpener(layer.id);
  return (
    <PromptScope layer={layer}><Primitive.Root open={open} onOpenChange={onOpenChange}>
      <Primitive.Portal>
        <Primitive.Overlay style={layer.style} className={overlayClasses} />
        <div style={layer.style} className="pointer-events-none fixed inset-0 z-(--layer-modal) flex items-start justify-center p-4 pt-(--palette-top)">
          <Primitive.Content
            data-prompt-id={layer.id}
            data-prompt-current={!layer.covered}
            aria-describedby={undefined}
            onCloseAutoFocus={(event) => { onCloseAutoFocus?.(event); if (!event.defaultPrevented) opener.giveBack(event); }}
            onOpenAutoFocus={(event) => {
              opener.remember();
              const input = (event.currentTarget as HTMLElement).querySelector<HTMLElement>('input');
              if (input === null) return;
              event.preventDefault();
              input.focus();
            }}
            className="pointer-events-auto flex max-h-(--dialog-max-height) w-full max-w-dialog-lg flex-col overflow-hidden rounded-xl border border-border bg-surface-raised shadow-lg data-[state=open]:animate-rise-in"
          >
            <Primitive.Title className={showTitle ? 'px-4 pt-3 text-base font-semibold' : 'sr-only'}>{label}</Primitive.Title>
            <ContentBoundary resetKey={open} className="m-3">
              {children}
            </ContentBoundary>
          </Primitive.Content>
        </div>
      </Primitive.Portal>
    </Primitive.Root></PromptScope>
  );
}
