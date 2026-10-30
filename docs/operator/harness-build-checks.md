# Check an agent build's history support

Harness-build checks run the exact program against a controlled model in an
isolated environment. Keep the complete result when choosing a recommended
build. A native-import refusal can pass its refusal check while leaving
`import_native` in the result's disabled features, with an `unsupported`
reason. It must write no session and submit no model request.

For a program that exports its history, a successful native import must export
two completed turns, start the same build in a fresh home, import its archive
and read both original answers. Codex exports no file, because it keeps a
thread's history in its own store. Its check makes three turns, forks the
conversation through the second turn in the home where it was made and reads
the fork back as exactly those two turns; the fork's next request must carry
them and not the third. It then stops the build, copies the home, starts the
same build on the copy and resumes the conversation: the next request must
carry all three answers. Every kept platform build of that kind and version
must supply that proof before Carry offers native import. Missing or incomplete evidence keeps it
off; rerun **Check** after replacing a build. A profile override cannot replace
this evidence. An unrelated optional-case failure does not erase a completed
native-import proof, but its own disabled features remain in force.

Users can read the selected method in their fork or switch plan. See
[Carry a thread to another agent](../user/carry-a-thread.md#read-the-method).
