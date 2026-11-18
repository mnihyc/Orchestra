/** Holder fields share their primitive without coupling their parent controllers to query rows. */
import type { Candidates } from '@/features/access/candidates';
import { ResourcePickerLookup, type ResourcePickerLookupProps } from '@/ui/ResourcePicker';

/** Every holder entrance shares Recent, keyboard behavior and its native paging/recovery footer. */
export function CandidatePicker({ candidates, groups = candidates.groups, ...props }: Omit<ResourcePickerLookupProps<string>, 'groups'> & {
  candidates: Candidates; groups?: ResourcePickerLookupProps<string>['groups'];
}) {
  return <ResourcePickerLookup {...props} pickerKey="permission-holders" groupHomes={false} filterLocal={false} groups={groups} footer={candidates.footer} />;
}

