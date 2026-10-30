# Person notifications

People can explicitly notify current readers of a document, chat, place or resource. A notice is checked again against the recipient's current access when it is read or delivered. Document and comment mentions do not send a notice by themselves.

`notifications.person_rate_per_hour` is a lockable policy from system through topic. It defaults to 120 recipients per sender and root org in a rolling hour; a resource without a root has its own system bucket. 0 disables the rate limit. `notifications.person_note_chars` defaults to 200 characters and is also lockable; 0 permits no note. Both values can be changed at the relevant scope. Successful acts are recorded in the audit trail without the note text.
