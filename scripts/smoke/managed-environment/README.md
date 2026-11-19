# managed-environment scenario

The whole system as an operator gets it and a person uses it. A release packaged from the tree is started by
the updater (`update.sh first-start --from`), `orchctl doctor` passes, the first administrator is made through
`/setup`, and then, over the same requests a browser makes:

1. the bundled host is online and usable: its page names everyone, so a person who is no administrator is
   listed the host and the Default template, may make an environment on it, and may not change it;
2. an org, a project and a topic; the scripted model server (a program on the host, on the bridge's address); a
   machine of the person (a container of the stack's network whose connector enrolled by a token, and which holds
   the provider and the harness); a provider registered from that machine on a key whose upstream is the model
   server, the key given to the system, and the models the upstream lists read back through the machine;
3. `POST /environments` from the Default template on the bundled host: the job goes through its phases, a
   container appears on the daemon, the program that runs in it is the connector of the release byte for byte,
   its connector enrolls, and the environment is `running`;
4. a thread bound to that environment on a profile whose harness never ran, a message, and a turn of the scripted
   agent that says where it works, asks for a command (the template lets it through), runs one, edits a file and
   answers, the items arriving on the realtime socket through the web edge; then the environment is stopped and
   started again, it is the same unit, and what was written in its workspace is still there; then, with its
   thread gone, it is removed, and its container and its volumes are gone.

The scripted agent only simulates what it runs and edits (it drives no harness), so the file the scenario looks
for after the restart is one the scenario writes into the workspace of the unit with `docker exec`, as the
operator can, which is what shows that the workspace outlives the unit's stop. A thread cannot be deleted by
itself yet, and an environment with a thread bound to it is not removed, so the thread goes with its topic; the
environment's home is the project, which stays.

The stack is the shipped one with three changes an operator makes for a host like this one: the pinned listener
has the same port inside and outside (the one machines of the stack's network and the managed units find it by,
through the name the system advertises), the system's name is made to be its own, so that what the node makes
for it can be found, and the public address is the web edge's name inside the stack, because the node downloads
the unit's connector from the address people reach the system at.

```sh
scripts/smoke/managed-environment/run.sh
```

It uses ports 41250 (the web edge, on the loopback), 41251 (the pinned listener, on the Docker bridge's address)
and 41252 (the scripted model server, on the bridge's address). Besides what the other scenarios need it needs
`jq` and `node`, and builds `orch-mock-llm` with the toolchain it packages with. At the end it takes down the
stack, the machine of the person and the model server, removes the units the node made for it and their
volumes, and the directory it worked in.
