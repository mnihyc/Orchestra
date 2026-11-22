/** Navigation dialogs with their own feature screens load when selected in the shell. */
import type { ComponentProps } from 'react';

import { lazyPart } from '@/features/files/lazy';
import type * as BindingDialogContents from '@/features/start/BindingDialog';
import type * as RecentlyDeletedContents from '@/features/navigation/RecentlyDeleted';
import type * as DocDialogsContents from '@/features/navigation/DocDialogs';
import type * as MoveContents from '@/features/org/Move';

export const NewThreadDialog = lazyPart<ComponentProps<typeof BindingDialogContents.NewThreadDialog>>(
  () => import('@/features/start/BindingDialog').then((module) => ({ default: module.NewThreadDialog })),
);

export const RecentlyDeleted = lazyPart<ComponentProps<typeof RecentlyDeletedContents.RecentlyDeleted>>(
  () => import('@/features/navigation/RecentlyDeleted').then((module) => ({ default: module.RecentlyDeleted })),
);

export const DeleteDocumentDialog = lazyPart<ComponentProps<typeof DocDialogsContents.DeleteDocumentDialog>>(
  () => import('@/features/navigation/DocDialogs').then((module) => ({ default: module.DeleteDocumentDialog })),
);

export const MoveDocumentDialog = lazyPart<ComponentProps<typeof DocDialogsContents.MoveDocumentDialog>>(
  () => import('@/features/navigation/DocDialogs').then((module) => ({ default: module.MoveDocumentDialog })),
);

export const RenameDocumentDialog = lazyPart<ComponentProps<typeof DocDialogsContents.RenameDocumentDialog>>(
  () => import('@/features/navigation/DocDialogs').then((module) => ({ default: module.RenameDocumentDialog })),
);

export const MoveDialog = lazyPart<ComponentProps<typeof MoveContents.MoveDialog>>(
  () => import('@/features/org/Move').then((module) => ({ default: module.MoveDialog })),
);
