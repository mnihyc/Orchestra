/** Workspace registrations and dialog hosts start when the reader’s frame is admitted. */
import { PanelTargetChoice } from '@/features/workbench/panelScope';
import { CloseViews } from '@/features/workbench/CloseViews';
import { WorkspaceCommands } from '@/features/workbench/workspaceActions';
import { useChatLinks } from '@/features/chat/links';
import { ExportHost } from '@/features/export/ExportHost';
import { ImportHost } from '@/features/people/ImportHost';
import { ShellCommands } from '@/features/finder/shellCommands';
import { TutorialHost } from '@/features/help/Tutorial';
import { AccessHost } from '@/features/access/AccessDialog';
import { usePortsRegistration } from '@/features/ports/View';
import { useBrowserRegistration } from '@/features/browser/View';
import { useVncRegistration } from '@/features/vnc/View';
import { DuplicateHost } from '@/features/duplicate/DuplicateHost';
import { ChangeHost } from '@/features/change/ChangeHost';
import { useRememberPlace } from '@/features/navigation/lastPlace';
import { NodeDialogHost } from '@/features/navigation/NodeDialogHost';
import { CapHost } from '@/features/usage/AddCap';
import { useVisitRecorder } from '@/features/navigation/visits';
import { useChatActivity } from '@/realtime/stores/chat';
import { Shortcuts } from '@/shell/Shortcuts';
import { HelpOverlayHost } from '@/features/help/HelpOverlay';

export interface FrameServicesProps {
  shortcutList: boolean;
  onShortcutList: (open: boolean) => void;
}

export default function FrameServices({ shortcutList, onShortcutList }: FrameServicesProps) {
  useChatLinks();
  usePortsRegistration();
  useBrowserRegistration();
  useVncRegistration();
  useVisitRecorder();
  useRememberPlace();
  useChatActivity();
  return <>
    <ShellCommands /><WorkspaceCommands /><TutorialHost />
    <HelpOverlayHost />
    <Shortcuts listOpen={shortcutList} onListOpenChange={onShortcutList} />
    <NodeDialogHost />
    <CapHost />
    <ChangeHost />
    <DuplicateHost />
    <AccessHost />
    <ExportHost /><ImportHost /><CloseViews /><PanelTargetChoice />
  </>;
}
