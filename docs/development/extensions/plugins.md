# External plugin transport

An external program implements one of provider, driver, adapter, executor, translator,
summarizer or notifier. Register it in its host component's local configuration:

```toml
[[plugin]]
kind = "summarizer"
name = "rules"
command = ["python3", "/opt/orch-plugins/summarizer.py"]
restart = "on-failure"
```

Give exactly one of `command` and `connect`. A command starts a child over
stdin/stdout and captures stderr separately. `connect = "unix:/run/rules.sock"`
attaches on Unix; `connect = "tcp://127.0.0.1:41880"` attaches over local TCP.
An attached service owns its process and environment; disconnecting the host
starts no replacement child. These local transports have no authentication and
accept no remote TCP address. Plugins run with the authority of their account.

The first JSON line is `hello`. The reply names the kind, negotiated transport
and contract version, implementation name/version, supported methods and typed
descriptor. Unknown members are tolerated; unknown methods return `unsupported`.
Required methods are checked before the plugin is usable. Every request has an
id, its response keeps that id, and overlapping replies may arrive in either
order. Progress does not extend a deadline. Cancel refers to one id; shutdown
ends a started program within its grace. The host pings idle plugins and
restarts or reconnects a lost one with bounded backoff.

Check before installing an entry:

```sh
orchctl plugin check python3 examples/plugins/summarizer/summarizer.py
orchctl plugin check --spec driver-spec.json python3 examples/plugins/driver/driver.py --base ./driver-check
orchctl plugin check --transport-only python3 examples/plugins/notifier/notifier.py --file ./notice-check.log
```

Kind comes from hello, or `--kind` for a kind-specific legacy handshake.
`--connect` checks a service that is already running. Driver checks require
`--spec FILE`; adapter, executor, notifier and translator checks require
`--fixture FILE` describing disposable contexts in their conformance module's
serialized `Fixture` shape. An executor isolation check starts two programs;
one attached service cannot certify two independent instances. A notifier
fixture must name a disposable destination because its checks actually deliver.

The command prints every named passed, failed or skipped rule and returns
status 1 when any fails. A missing context is an error. `--transport-only`
explicitly leaves family behavior unchecked; a skipped optional rule is no
native proof. Install the checked program and edit its host's local entry, then
restart that component using its normal operator procedure. The check itself
installs nothing and changes no configuration.

The async bridges work on a current-thread runtime. Synchronous translation
and notifier link generation require a multithread runtime or an outside
calling thread; the former is refused immediately on a current-thread runtime.
Rust built-ins and external bridges use the same trait suites, with native
behavior tests retained alongside them.

Provider programs and addresses implement the same facts, models and reset
contracts; local programs may also present keys. Start with the
[provider source overview](providers/README.md) and its individual interface
pages. A managed provider's trusted template carries plugin registrations
as `plugins`, passed to the connector in `ORCH_PLUGINS`; all four connector
kinds (provider, translator, adapter, executor) load. Its image must include
the executable. No provider API installs a program.

`orchctl plugin check --address URL --token-file FILE --fixture FILE` checks
an authorized reporting address through the provider suite. HTTPS is normal;
plain HTTP requires the existing upstream policy and says Unencrypted.
Provider fixtures use disposable accounts and explicitly permit any named
test reset they consume. Address presentation is unsupported. Read the
[facts](providers/facts.md), [models](providers/models.md),
[resets](providers/resets.md) and [key presentation](providers/credential.md)
pages for generated schemas, members, bounds and exact worked examples.
