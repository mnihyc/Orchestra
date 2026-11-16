---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: Every request option a provider endpoint accepts, and what it changes.
---

# Option field reference

An option is offered only where its interface or program declares it. Provider limits and permissions still apply. Fields you define in a custom schema keep that schema’s own meaning; this reference covers the shipped declarations. Setting fields are in the [settings reference](settings-reference.md). Read the plan before a change and [the program guide](register-your-program.md#choose-an-arrangement) before choosing an arrangement.

## Entries

| Reference | Meaning |
| --- | --- |
| `option:McpBinding.server_id` | The same registered MCP UUID carried by the machine-link binding; the adapter-facing mirror preserves it through a read/write round trip. |
| `option:Job.kind` | A mount_copy job performs a reviewed standalone data copy through the normal progress and outcome rows; server_update runs the release updater on the bundled node. Notify on mount_copy keeps one renewable Inbox notice per affected person through progress and outcome; leaving it off sends none. Copy progress names a waiting machine only when the current reader may read that machine. |
| `option:JobProgress.message` | `mount_copy_custody_pending` means cleanup keeps the job running until remaining copy data has a durable kept-data record. Its progress line and tooltip use the reader’s language. |
| `option:MountCopyPreflightBody.source_machine_id` | The existing machine holding this mount location; the server retains its exact membership, path or owned volume. |
| `option:MountCopyPreflightBody.target_machine_id` | The allowed destination machine; the server chooses native storage from its current data-making declaration. |
| `option:MountCopyPreflightBody.into` | Replace or New location when the destination already holds data; either choice keeps the displaced data. |
| `option:MountCopyPlan.preflight_id` | This person’s saved copy review, required for acceptance and scoped recipient inspection. |
| `option:MountCopyPlan.expires_at` | When this review expires; a changed native source requires a fresh review sooner. |
| `option:MountCopyPlan.mount` | The current readable mount being copied. |
| `option:MountCopyPlan.source` | The original readable machine from which data is copied. |
| `option:MountCopyPlan.target` | The readable destination machine. |
| `option:MountCopyPlan.copy` | The shared measured copy plan, including destination data and its preservation; unfinished counts remain unknown. |
| `option:MountCopyPlan.blockers` | Current reasons that stop acceptance; reviewing a blocked copy mutates no data. |
| `option:MountCopyPlan.notification` | The common explicit Notify choice and unique affected-person count. |
| `option:MountCopyPlanReceipt.projection` | A restricted acknowledgement of the saved review, without existing resource content. |
| `option:MountCopyPlanReceipt.mount_id` | The exact mount named by this request. |
| `option:MountCopyPlanReceipt.preflight_id` | The saved review identity, without its measured or private location details. |
| `option:MountCopyPlanReceipt.expires_at` | The deadline to accept this review. |
| `option:MountCopyCreateBody.preflight_id` | The exact current review to accept; repeating its acceptance joins the durable job. |
| `option:MountCopyCreateBody.notify` | The explicit notification choice; omission follows the reviewed default and recipients are grouped once per operation. |
| `option:AdapterEvent.native_opening_id` | The producer's physical opening, captured from its admitted SessionSpec before output waits; null is reserved for unscoped history and cannot control a live session. |
| `option:ModelRequestReport.native_opening_id` | The physical opening that observed a request; delivery and replay preserve it, and measured usage remains attached to its original logical turn. |
| `option:Ev.native_opening_id` | The original producer carried beside a durable session event; earlier reports retain their facts without controlling a replacement. |
| `option:EvbEntry.native_opening_id` | Each batched observation keeps its own producer; batching, coalescing and restart cannot relabel it from a current session. |
| `option:SessionOpenMcp.server_id` | Every offered session MCP server carries its registered UUID alongside its current name; a name alone is refused. |
| `option:Cmd.native_members` | The member names and nonces frozen with an accepted session configuration. MCP and model changes compare their own members at the native effect boundary; another independent member can change without blocking them. |
| `option:Cmd.native_revision` | The factual revision captured with the original session members. A retry retains it; delivery does not stamp the command from a newer configuration. |
| `option:ToolSessionControl.refreshing` | A primary is synchronizing current scope facts. Local access waits; independent retained remote carriers keep their own live relay admission. |
| `option:Frame.tool_session_update` | Committed session and member withdrawals reach only the original source machine’s authenticated primary connection; the notice installs no native route. |
| `option:Frame.tool_session_control` | Current API control availability suspends local tool exchanges while retaining their native conversation. |
| `option:ToolSessionUpdate.machine_id` | The original source machine of this physical opening; another machine’s primary or a tunnel-only attachment cannot receive its native control. |
| `option:ToolSessionUpdate.thread_id` | The exact thread whose committed physical scope changed. |
| `option:ToolSessionUpdate.segment_id` | The original logical segment; an update never selects a newer segment as its target. |
| `option:ToolSessionUpdate.opening_id` | The physical opening whose access changed, including when its segment is removed. |
| `option:ToolSessionUpdate.revision` | Monotone membership facts; an older notice cannot restore access or retire a successor. |
| `option:ToolSessionUpdate.live` | False withdraws the whole opening; true preserves only the existing members whose nonces still match. |
| `option:ToolSessionUpdate.members` | The remaining factual member names and nonces; this list grants no new route or permission. |
| `option:ToolSessionMember.member` | The exec, llm or stable MCP member identity within one physical opening. |
| `option:ToolSessionMember.nonce` | An unchanged member’s identity; removing and adding it again produces a different nonce. |
| `option:ToolSessionControl.available` | Whether the live API can currently validate this installation’s session tools. |
| `option:ToolSessionControl.admission_timeout_ms` | The positive configurable allowance for an API admission question, from tunnels.control_timeout_ms. |
| `option:ToolSessionQuery.n` | One pending admission question; a later answer cannot be reused for another channel. |
| `option:ToolSessionQuery.claims` | The exact original verified ticket claims presented for Open or Resume. |
| `option:ToolSessionQuery.ticket` | The original signed ticket, verified again by the API and never included in diagnostic output. |
| `option:ToolSessionReply.n` | The one pending question this answer settles, once. |
| `option:ToolSessionReply.allowed` | Whether the current native opening and member admit this exact request. |
| `option:ToolSessionReply.revision` | The committed native revision actually read for that admission. |
| `option:Cmd.native_target` | The original physical operand accepted for a native act. A known absent session remains absent for that act; later execution never looks up a replacement. It grants no permission. |
| `option:NativeTarget.session` | A captured session operand carried by the current machine-link protocol; an older receiver cannot silently ignore it. |
| `option:SessionTarget.thread_id` | The exact thread of the accepted native operation, matched against its ordinary command body. |
| `option:SessionTarget.segment_id` | Its captured logical segment, or null when the act was accepted without one. |
| `option:SessionTarget.opening_id` | The immutable physical opening; null records an already absent target, rather than a request to close a later session. |
| `option:SessionTarget.opening_epoch` | The accepted opening order across a thread’s segments, independent from timestamps and process IDs. Native retirement keeps that order across an empty slot and restart. |
| `option:ToolSessionScope.opening_epoch` | The signed accepted opening order; an older segment cannot regain access after a provisional successor is removed. |
| `option:ToolSessionUpdate.opening_epoch` | Orders physical opening changes across logical segments; a delayed older update cannot close its successor. |
| `option:McpServerCreateBody.name` | The added server's name; exec and orch belong to builtin tool servers and cannot name an added definition. |
| `option:EnvironmentMcpServerCreateBody.name` | The environment-owned server's name, distinct from the builtin exec and orch services. |
| `option:SessionSpec.opening_id` | The accepted physical opening command passed to the adapter; it is separate from logical settings and from the private loopback key. |
| `option:SessionOpened.opening_id` | The physical opening that produced this report. A delayed earlier report cannot fill or reopen its replacement. |
| `option:SessionLost.expected` | A planned idle retirement closes the old physical opening quietly while retaining its conversation. A turn that was running still receives its actual loss and recovery outcome. |
| `option:SessionLost.opening_id` | The physical opening that actually ended, also retained across connector restart; only its matching live admission closes and recovers. |
| `option:TicketClaims.tool_session` | The immutable physical opening, segment, thread, member nonce and admitted revision of a session tool ticket; every Open and Resume also asks the API for current admission. |
| `option:ToolSessionScope.opening_id` | The accepted physical opening command; a reopen takes a new identity and transport refresh preserves the existing one. |
| `option:ToolSessionScope.member` | The native exec member (including the builtin MCP executor transport), the llm member, or mcp followed by the stable server UUID; a server name cannot stand for its identity. |
| `option:ToolSessionScope.nonce` | The unchanged member keeps its nonce; withdrawal and re-add issue a new one, leaving other members usable. |
| `option:ToolSessionScope.revision` | The durable ordering of session and membership facts; older replies cannot restore a closed or withdrawn channel. |
| `option:ToolSessionScope.thread_id` | The exact thread of this physical session, checked against both the signed ticket and the current segment. |
| `option:ToolSessionScope.segment_id` | The existing logical segment containing the physical opening; it alone does not identify a reopen. |
| `option:CleanupPlan.effects` | Current consequences from the shared usage-facts service: mount references switch off now; existing environment attachments end at their next start. Names follow current visibility; hidden consumers remain counts. |
| `option:CleanupStart.plan_id` | The exact current preview read by this person; replacing a mount location invalidates its old preview even at the same timestamp. |
| `option:CleanupStart.choices` | One displayed fate per selected item. Mount Delete can proceed with every item kept, including an empty mount. |
| `option:CleanupStart.confirmation_name` | The exact mount or plan name when selected owned data is destroyed; keeping data never asks for a typed name. |
| `option:CleanupStart.notify` | An explicit notification choice for the whole operation, narrowed by each recipient's preferences and grouped once per person. |
| `option:CleanupItem.record.blocker` | The actual reason a physical fate is unavailable, including not_permitted when data-management authority is absent; deleting the definition and keeping data remain possible. |
| `option:NotificationOperation.kind` | mount_delete identifies the one accepted mount deletion and updates its existing grouped notice as its shared cleanup job ends. |
| `option:ServerUpdateStart.release` | The exact currently offered semantic release selected by a system administrator; no path, command, URL or argument list is accepted. |
| `option:ServerUpdateView.installed_version` | The release of the API answering this read, including after an update or rollback. |
| `option:ServerUpdateView.offered_version` | The last validated release offer, or null before one is available. |
| `option:ServerUpdateView.update_available` | Whether that offer is newer than the API answering now; a cached earlier check never claims the new server is still behind. |
| `option:ServerUpdateView.availability` | The actual bundled-node, connection and local Docker condition for the Management update action. |
| `option:ServerUpdateView.command` | The same update.sh operator command remains available when the native bundled-node path cannot run. |
| `option:ServerUpdateView.job` | The most recent durable server update job, retained across API restart and page reload; another press joins an open job. |
| `option:ServerUpdateView.observation` | The latest safe phase and outcome recorded by the host updater, without native logs or credentials. |
| `option:ServerUpdateObservation.stage` | The phase recorded by the release updater; updated and rolled_back are confirmed outcomes. |
| `option:ServerUpdateObservation.reason` | A fixed safe failure reason, separate from the phase and from unfiltered native output. |
| `option:ServerUpdateObservation.from_version` | The release being left, once the existing updater journal records it. |
| `option:ServerUpdateObservation.to_version` | The exact release admitted for this job, unchanged by reconnection or another release check. |
| `option:NodeServerUpdate.timeout_secs` | The positive native preparation allowance captured from server.update_prepare_timeout_secs; it overrides the command kind’s default and does not time-limit the detached release transaction. |
| `option:NodeServerUpdate.command_id` | The exact successful native command behind this job’s prepared helper; observations without the same stored job, machine, release and private source are ignored. |
| `option:NodeServerUpdate.job_id` | The same durable update job on its authenticated bundled node; another node cannot supply its outcome. |
| `option:NodeServerUpdate.release` | The exact validated release for the fixed installation updater; native paths and host commands are never request fields. |
| `option:NodeServerUpdate.observation` | The bundled node reports only its own operation’s safe updater record through its durable node stream. |
| `option:MachineFacts.server_update` | A locally reported fixed updater capability, present only on the physical bundled installation with local Docker access. |
| `option:AuditAction.server.update_requested` | Records who requested the server release, its durable job and the exact bundled host and node. |
| `option:update.sh.ORCH_UPDATE_JOB` | The native helper attaches its durable job UUID for phase recording and recovery. Ordinary shell updates omit it; it contains no credential and chooses no host command. |
| `error:server_update_rolled_back` | The selected release failed readiness and the existing updater confirmed that the earlier release is running again. |
| `error:server_update_failed` | The existing updater could not finish; the retained observation says whether the original recovery command is needed. |
| `state:ServerUpdateAvailability.ready` | The connected bundled node offers the fixed local updater. |
| `state:ServerUpdateAvailability.no_bundled_node` | No active node is marked as bundled with the server; use the shown shell command. |
| `state:ServerUpdateAvailability.ambiguous_bundled_nodes` | More than one active node claims the server bundle, so no host is guessed. |
| `state:ServerUpdateAvailability.node_offline` | The bundled node must reconnect before it can receive an update job. |
| `state:ServerUpdateAvailability.docker_unavailable` | The bundled node lacks the installation’s local Docker updater path; the operator command remains. |
| `state:ServerUpdateStage.queued` | The admitted job has not started fetching. |
| `state:ServerUpdateStage.fetching` | The existing updater is fetching its manifest and assets. |
| `state:ServerUpdateStage.verifying` | The fetched release has reached its native verification step. |
| `state:ServerUpdateStage.applying` | The verified payload generations are being prepared. |
| `state:ServerUpdateStage.stopping` | The application services are stopping for the transaction. |
| `state:ServerUpdateStage.dumping` | The existing updater is taking the rollback database snapshot. |
| `state:ServerUpdateStage.activating` | The existing updater is selecting the new payload generations. |
| `state:ServerUpdateStage.starting` | The application services are restarting on the selected release. |
| `state:ServerUpdateStage.checking` | The existing updater is probing service and browser-route readiness. |
| `state:ServerUpdateStage.rolling_back` | The existing updater is restoring the previous payloads and database. |
| `state:ServerUpdateStage.recovering` | The same operation is recovering its existing journal after interruption. |
| `state:ServerUpdateStage.updated` | The selected release is running and passed readiness. |
| `state:ServerUpdateStage.rolled_back` | The previous release is confirmed running again after the attempted update. |
| `state:ServerUpdateStage.failed` | The updater did not confirm an updated or restored release; retain its recovery path. |
| `state:ServerUpdateFailure.health_check_failed` | The selected release did not pass service or browser readiness. |
| `state:ServerUpdateFailure.update_failed` | The updater ended before confirming its requested release or rollback. |
| `state:ServerUpdateFailure.recovery_required` | The original journal remains and the same update.sh recover path applies. |
| `option:PublicAddressUpdateBody.expected_revision` | The applied address revision used to prepare this draft. A concurrent publication or local reset refuses the stale draft without returning another system projection. |
| `option:PublicAddressUpdateBody.public_url` | The complete proposed primary origin, published only after certificate and installation checks succeed. |
| `option:PublicAddressUpdateBody.alternate_urls` | The complete proposed alternate origin set; omission is invalid, and an empty list clears it atomically with the primary. |
| `option:PublicAddressSnapshot.using_example` | The effective primary is still the shipped example address, rather than an installation-specific browser origin. |
| `option:PublicAddressUpdated.projection` | An intrinsic receipt of a prepared address publication. It carries no prior file seed or general system configuration. |
| `option:PublicAddressUpdated.revision` | The revision committed by this publication. |
| `option:PublicAddressUpdated.public_url` | The accepted canonical primary origin, derived from this caller’s draft. |
| `option:PublicAddressUpdated.alternate_urls` | The accepted alternate origins, normalized together with the primary. |
| `option:PublicAddressUpdated.using_example` | Whether the accepted primary is the shipped example. |
| `option:Meta.public_address_revision` | The committed revision read atomically with public_url and alternate_urls. A changed revision refreshes readers and a one-line browser notice, without reloading other browsers. |
| `option:orchctl.public-address.reset` | Restore the validated file primary and alternate seed through the local database, clear the stored override and advance its revision. Restoring an already admitted seed or current address set needs no HTTP hostname. A novel file origin uses the same certificate/origin preparation before commit. No credential is printed. |
| `option:orchctl.public-address.get` | Print only the committed effective primary address for local scripts. The file is used only if the system store has not been seeded; later file edits do not replace it. |
| `option:orchctl.doctor.only` | Includes public-address for the stored primary, alternates, revision and seed/override state. Without an explicit --url, selected browser checks use the committed address; default local checks probe the page and API. |
| `option:headers.X-Orch-Frontend-Generation` | The shipped frontend’s loaded worker certificate generation. Publication verifies this marker, the served TLS leaf and the installation proof before changing the effective addresses. |
| `option:PublicAddressProofQuery.challenge` | A fresh noncredential UUID challenge for one browser-origin check. |
| `option:PublicAddressProof.challenge` | The exact challenge given to this origin; a proof for another attempt is refused. |
| `option:PublicAddressProof.proof` | A purpose-bound keyed proof from this installation’s existing private authority. It signs nobody in and reveals no key or credential. |
| `option:PublicAddresses.public_url` | The primary HTTP(S) browser origin, validated with its alternates and read from one committed system-store revision for new links, commands and deliveries. |
| `option:PublicAddresses.alternate_urls` | Other explicit browser origins of this installation. Publication replaces the whole set atomically; a file edit does not replace an already seeded value. |
| `option:PublicAddressSnapshot.public_url` | The primary address in force at this revision. Already admitted logins, issued claims and exported packages keep their admitted address. |
| `option:PublicAddressSnapshot.alternate_urls` | The alternate origins committed together with the primary address. |
| `option:PublicAddressSnapshot.revision` | The monotone revision checked before publishing a prepared address candidate; a stale draft changes nothing. |
| `option:PublicAddressSnapshot.from_file_seed` | The stored initial seed is in force. The configuration file is read only to create the first snapshot or for an explicit local reset. |
| `option:MachineGatewaySetBody.selection` | Choose Automatic or a selected advertised gateway without reenrolling or replacing the machine identity. |
| `option:MachineGatewaySetBody.gateway_id` | Required only for Selected; Automatic omits or clears this value. Addresses outside the installation catalogue are refused. |
| `option:MachineGatewaySetReceipt.projection` | A receipt for the accepted native choice, without existing machine content. |
| `option:MachineGatewaySetReceipt.machine_id` | The same enrolled machine named by the request. |
| `option:MachineGatewaySetReceipt.selection` | The accepted primary selection; actual connection state remains in gateway status. |
| `option:MachineGatewaySetReceipt.gateway_id` | The selected advertised gateway identity, or null for Automatic. |
| `option:MachineGatewaySetReceipt.changed` | Whether this native write changed the saved choice; its reply is journaled before reconnecting. |
| `option:MachineGatewaySet.choice` | Native primary selection, validated against the machine’s deployment-bound advertisement. |
| `option:MachineGatewaySetResult.choice` | The exact native choice that was saved. |
| `option:MachineGatewaySetResult.changed` | Whether the saved configuration changed; this does not claim the new connection is already online. |
| `option:AuditAction.machine.gateway_changed` | Records who changed the primary gateway, its machine, saved choice and native command. |
| `option:Host.bundled_with_server` | Immutable origin recorded only by the protected server bootstrap token; never inferred from a host name or chosen home. |
| `option:PagePreview.server_host_use` | Current warning and exact acknowledgement for a Host Use, Manage or ownership grant on the host installed with the server; absent when it gains no new grant. |
| `option:ServerHostUseDisclosure.hosts` | How many protected server hosts gain or change a native grant through this page or its defaults; no host identities or names are disclosed. |
| `option:ServerHostUseDisclosure.confirmation` | Acknowledge the exact permission change and current native grants with the orch-server-host-use-confirmed header. Changed intent asks again; this proof grants no access. |
| `option:ProviderModelPauseQuery.note` | A short note beside the disabled model, visible in its affected threads. |
| `option:ProviderModelPauseQuery.run_waiting` | Enable runs eligible waiting messages only with true; omitted or false keeps them held for Resume. |
| `option:ProviderModelPauseQuery.notify` | Disable explicitly sends one renewable lifecycle notice per affected person for this exact model; omitted is silent. The dialog starts from Operation notices. |
| `option:Notification.data` | Model lifecycle notices use operation_kind provider_model_disable or provider_model_delete with running, done or failed; the same grouped item records progress and outcome. |
| `option:ProviderPauseQuery.note` | A short note beside the provider stop, visible in affected threads. |
| `option:ProviderPauseQuery.notify` | Explicitly send one grouped lifecycle notice per affected person when disabling a provider; omitted is silent. The dialog begins with Operation notices and lets the confirmer change it. |
| `option:ResourceDeleteQuery.notify` | Explicitly send one grouped lifecycle notice per affected person when deleting a provider, one of its models or an MCP definition; omitted is silent. Provider child-job progress updates the same Inbox item. MCP removal records the definition change while durable machine cleanup continues. |
| `option:ResourceImpact.notification` | The current notification audience and initial checkbox value for a provider-wide disable, provider deletion or MCP definition deletion; absent for other impact kinds and individual models. |
| `option:ResourceOperationNotification.audience` | Visible affected people and the count whose association remains private; the confirming person is excluded and each person is counted once. |
| `option:ResourceOperationNotification.default_on` | The inherited Operation notices default for this consequence; the confirmer may change the choice before acting. |
| `option:ask.models.*.model` | The enabled API model’s routing name. Together with provider_id it identifies one allowed Ask model; child lists can narrow the choices while retaining the parent’s order. |
| `option:ask.models.*.provider_id` | The API provider’s identity for this allowed Ask model. The person and current payer must still be allowed to use that provider and model in the conversation’s organization. |
| `option:AccessOptions.models` | The models. |
| `option:AccessOptions.services` | Enabled provider services this Use grant allows, by name, family or group. Omitted permits every enabled service; this list is independent of generation models. |
| `option:AccessOptions.efforts` | The efforts. |
| `option:AccessOptions.where` | The environment, or the host or machine of one that is to be made, that a refused use was asked for; it tells where the provider's entry does not reach. |
| `option:BindingOptions.viewer_default` | The current creation default and its permitted provenance; absent when rebinding an existing thread. |
| `option:BindingOptions.environment_readers` | The exact prospective page's View offer on the selected existing environment. A new environment is checked in the creator transaction before the thread is made. |
| `option:BindingOptions.defaults` | What settings choose. |
| `option:BindingOptions.providers` | The providers, each with its models and agents. |
| `option:BindingOptions.environments` | Every existing environment the caller manages. This topic's come first (its default, then those its threads use or that were made for it), and the rest follow. |
| `option:BindingOptions.environment_default` | What a new thread does about its environment unless told. |
| `option:BindingOptions.default_environment` | The environment the nearest setting names as the default, wherever it is set, and why the reader cannot use it when they cannot; absent when no setting names one and when an existing thread is bound again. |
| `option:BindingOptions.source` | Safe resolved repository settings and the place supplying the repository address. |
| `option:BindingOptions.access` | The permission page that a thread created here would begin with. |
| `option:BindingOptions.agent_access` | Agent privilege choices for this creation context. Rebinding an existing holder does not create another contribution and leaves this field absent. |
| `option:BindingOptions.new_environment` | Who would be able to see, and who to manage, an environment made now for this topic. |
| `option:BindingOptions.can_create` | Whether the caller may make a new environment. |
| `option:BindingOptions.hosts` | The hosts a new environment for a thread here could be made on, the one with the most free memory first: what each has, what is left of it, whether it can be chosen and why not, what it warns about, and whether the scheduler would choose it. Empty where the caller may make no environment here or the place names no template. |
| `option:BindingOptions.machines` | The caller's own connected machines: a thread runs on one through an environment that is a directory there. |
| `option:ChangeOptions.active_turn` | How to treat a turn that is under way. |
| `option:ChangeOptions.wait_minutes` | With waiting: how long to wait before giving up with nothing changed. |
| `option:ChangeOptions.verify_timeout_secs` | How long the health checks may take. |
| `option:ChangeOptions.keep_previous` | Keep what is left behind rather than scheduling it for removal. |
| `option:ChangeOptions.stop_processes` | For a switch out of an environment that others stay in: end this thread's processes there. |
| `option:AppliedChangeOptions.active_turn` | How to treat a turn that is under way. |
| `option:AppliedChangeOptions.wait_minutes` | How long to wait for it. |
| `option:AppliedChangeOptions.verify_timeout_secs` | How long the health checks may take. |
| `option:AppliedChangeOptions.keep_previous` | Whether what is left behind is kept. |
| `option:AppliedChangeOptions.stop_processes` | Whether this thread's processes are ended. |
| `option:ImportOptions.parts` | Which kinds of thing; all three unless given. |
| `option:ImportOptions.versions` | The earlier versions of documents, where the package has them; `true` unless given. |
| `option:ImportOptions.comments` | The discussions of documents; `true` unless given. |
| `option:ImportOptions.access` | Whether the sharing the package carries is restored, within what the importer holds and what the place's locks allow; `false` unless given, and refused for one who does not hold `access.manage` there. |
| `option:ImportOptions.authors` | By whom things are written: the accounts for an Admin of the place, and the importer otherwise. |
| `option:ImportOptions.same_source` | What is done with an object that an earlier import of the same thing made: `replace` unless given. |
| `option:ImportOptions.same_name` | What is done with an object whose name another of the place has: `keep_both` unless given. |
| `option:ParameterOptions.from_model` | Which list. |
| `option:protocols.openai-responses.web_search.search_context_size` | The amount of search context requested: low, medium or high. |
| `option:protocols.openai-responses.web_search.return_token_budget` | The declared return-token budget: default or unlimited. |
| `option:protocols.openai-responses.web_search.filters` | Domain filters for this search; allow and block lists cannot be supplied together. |
| `option:protocols.openai-responses.web_search.filters.allowed_domains` | Only search the listed domains. |
| `option:protocols.openai-responses.web_search.filters.blocked_domains` | Do not search the listed domains. |
| `option:protocols.openai-responses.web_search.user_location` | Approximate location supplied for this search; it is a provider option, never inferred from the reader. |
| `option:protocols.openai-responses.web_search.user_location.type` | The approximate-location kind. |
| `option:protocols.openai-responses.web_search.user_location.city` | City used for the approximate search location. |
| `option:protocols.openai-responses.web_search.user_location.region` | Region used for the approximate search location. |
| `option:protocols.openai-responses.web_search.user_location.country` | Country code used for the approximate search location. |
| `option:protocols.openai-responses.web_search.user_location.timezone` | Time zone used for the approximate search location. |
| `option:protocols.openai-responses.native_tools.image_generation.request.model` | The image model used by the hosted tool, separate from the conversation model. Choose a model the endpoint supports within its allowed options. |
| `option:protocols.openai-responses.native_tools.image_generation.request.action` | Choose auto, generate or edit. Auto lets the tool choose; editing still needs the image references supplied with the request. |
| `option:protocols.openai-responses.native_tools.image_generation.request.background` | Choose auto, opaque or transparent. The selected image model and output format must support the requested background. |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_compression` | The provider’s compression setting from 0 to 100 for JPEG or WebP output, where supported; it does not change the requested image count. |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_format` | The requested image encoding: PNG, JPEG or WebP. This does not prove the format of returned bytes; the result keeps its reported format. |
| `option:protocols.openai-responses.native_tools.image_generation.request.partial_images` | Request 0 to 3 streaming previews. A preview is not another completed image; the provider may include preview work in its token usage. |
| `option:protocols.openai-responses.native_tools.image_generation.request.quality` | The requested rendering quality. Use only levels supported by the selected image model and its option constraints. |
| `option:protocols.openai-responses.native_tools.image_generation.request.size` | The requested dimensions or auto, within the selected image model’s supported sizes. It does not override provider limits. |
| `option:protocols.anthropic-messages.web_search.max_uses` | Maximum search-tool uses in this request, when set. |
| `option:protocols.anthropic-messages.web_search.allowed_domains` | Only search the listed domains. |
| `option:protocols.anthropic-messages.web_search.blocked_domains` | Do not search the listed domains. |
| `option:protocols.anthropic-messages.web_search.user_location` | Approximate location supplied for this search; it is a provider option, never inferred from the reader. |
| `option:protocols.anthropic-messages.web_search.user_location.type` | The approximate-location kind. |
| `option:protocols.anthropic-messages.web_search.user_location.city` | City used for the approximate search location. |
| `option:protocols.anthropic-messages.web_search.user_location.region` | Region used for the approximate search location. |
| `option:protocols.anthropic-messages.web_search.user_location.country` | Country code used for the approximate search location. |
| `option:protocols.anthropic-messages.web_search.user_location.timezone` | Time zone used for the approximate search location. |
| `option:SummaryTargetOptions.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:SummaryTargetOptions.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:SummaryTargetOptions.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:SummaryTargetOptions.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:SummaryTargetOptions.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:SummaryOptionsBody.options` | The target’s own options; null removes the override and restores inheritance. |
| `option:SummaryOptionsBody.expected_revision` | The options revision read by the caller, so a changed choice must be reviewed again. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.max_uses` | Maximum page-fetch tool uses in this request, when set. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.allowed_domains` | Only fetch pages from the listed domains; it cannot be set together with blocked domains. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.blocked_domains` | Never fetch pages from the listed domains; it cannot be set together with allowed domains. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.max_content_tokens` | The largest page content, in tokens, the provider passes to the model for one fetch, when set. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.citations` | Whether fetched pages are returned with citations the answer can point to. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.citations.enabled` | Turns citations for fetched pages on or off. |
| `option:ModelGroupOptions.request_options` | Request option forms and constraints keyed by interface; `native` names program parameters. The endpoint, group and offering constraints still apply. |
| `option:providers.presets.*.credential_hints` | Public key-format hints that propose this preset and a credential role. They never verify a key or enable an offering; the normal first read after saving confirms the vendor facts. |
| `option:providers.presets.*.credential_hints.*.prefix` | The plain public beginning of a key format. Matching only proposes a choice; the complete key stays in the form until Save. |
| `option:providers.presets.*.credential_hints.*.role` | The proposed role: model access or an admin credential for the facts source. A role the client does not recognize requires an explicit supported choice. |
| `option:providers.presets.*.credential_hints.*.field` | Optional declared secret-field name for the proposed admin credential in the selected facts source. It is kept separate from model access and must be offered by that source. |
