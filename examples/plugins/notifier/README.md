# External notifier example

A notifier for Orch that appends what it is asked to deliver to a file, one line each. It is a destination: a node
can be told at it. Its one field is a label, written before each line, so that destinations can be told apart. A
delivery that is tried again is not written twice, since the delivery's id is the same in every try.

It shows what a notifier program has to do and nothing else: answer `hello` with its versions, its methods and its
descriptor (what it reaches, its fields and the texts it ships), look at the values of a destination when they are
saved (`check`), and `deliver`. A notifier that links people to a bot would also implement `link_url` and
`poll_links`; this one declares neither, and is never asked for them.

| File | Role |
|------|------|
| [notifier.py](notifier.py) | The program. |

The file is named by the operator who registers the program, in the configuration of the worker, and not by the
people who set up destinations, so that a destination cannot make the worker write wherever it likes:

```toml
[[plugin]]
kind = "notifier"
name = "file"
command = ["/opt/orch-plugins/notifier.py", "--file", "/var/log/orch-notices.log"]
```

Protocol types belong to `crates/contracts/plugin` and `crates/contracts/notifier`; `crates/platform/plugin` is the
host's side, and runs this program in its tests.
