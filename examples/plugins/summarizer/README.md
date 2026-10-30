# External summarizer example

A summarizer for Orch that needs no model. It turns the activity of a node into the text of one of its fields by
three fixed rules, which is as valid as asking a model:

| Field | Rule |
|-------|------|
| `title` | The first thing a person said, cut to the limit. |
| `description` | The last thing that was said or done, after the optional `prefix` of its configuration. |
| `tags` | The offered tags whose names occur in the activity, at most as many as the limit. |

When nothing happened since the last summary it answers `skipped`, which is no failure. It shows everything a
summarizer program has to do: answer `hello` with its versions, its methods and its descriptor, and answer
`summarize`.

| File | Role |
|------|------|
| [summarizer.py](summarizer.py) | The program. |

Register it in the configuration of the API server, and name it in the setting `auto.summarizer`:

```toml
[[plugin]]
kind = "summarizer"
name = "rules"
command = ["/opt/orch-plugins/summarizer.py"]
```

It can be tried by hand, a line at a time:

```sh
python3 summarizer.py
{"id":0,"method":"hello","params":{"kind":"summarizer","transport":[1,1],"contract":[1,1],"host_version":"0.1.0"}}
```

Protocol types belong to `crates/contracts/plugin` and `crates/contracts/summarizer`; `crates/platform/plugin` is the
host's side, and runs this program in its tests.
