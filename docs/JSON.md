# `--json` output

Every command prints exactly one JSON object on one line to stdout, and nothing to stderr. Written by `cigi-ui` (`Ui::finish`); flows only fill `Report::data`.

## Envelope

Success (`Report` with a done, running-free outcome):

```json
{"ok":true,"command":"update","headline":"...","detail":["..."],"next":"...","code":null,"data":{}}
```

`next` and `code` are `null` when absent. `ok` is `false` for a refused or bug report (a `check` that found a refusal); then `code` is the finding's code and the exit code is 2 (70 for a bug).

Failure (`InstallerError`):

```json
{"ok":false,"command":"carton","error":{"code":"E1811","kind":"usage","message":"...","hint":"...","subject":"1.4"}}
```

- `kind` is `usage`, `runtime` or `internal`, from the registry range of the code, so cig's `diagnostic.schema.json` accepts the `error` object unchanged.
- `hint` is what to type next (cig's meaning): the flow's complete fix command, else the registry's `fix`. `subject` is omitted when absent.
- The process exit code is the error's `exit` (SPEC 11.2).

Without `command` (when `Ui::error` is used instead of `Ui::finish`), the `command` key is omitted.

## `data` shapes

Keys are stable once released; new keys may be added. Versions are strings; paths are absolute; times are RFC 3339 UTC. Flows build these with `serde_json::json!`; each command has a golden test once its flow exists (`install`, `which`, `update`, `finding` and `status` envelopes are golden today in `crates/cigi-ui/tests/golden/*.json.txt`).

| command | `data` |
|---|---|
| (none), status | `{"carton":"0.1.0","channel":"stable","tools":{"cig":"1.20.0"},"trust":{"pinned":1,"checksummed":2},"waiting":{"update":"0.1.1"\|null,"advisories":[],"yanked":[]}}` |
| `install` | `{"carton":"0.1.0","tools":{"cig":"1.20.0"},"trust":{"pinned":1,"attested":0,"checksummed":2},"path_block":"added"\|"present"\|"skipped","txn":"<id>","dry_run":false}` |
| `carton` | `{"carton":"0.1.0","newer":"0.1.1"\|null,"channel":"stable"}` |
| `carton <v>`, `use`, `rollback` | `{"from":"0.1.0","to":"0.1.1","trial":{"scripts":14,"same":14,"differ":0}\|null}` |
| `carton list` | `{"channel":"stable","cartons":[{"version":"0.1.1","installed":true,"yanked":false,"changes":[{"tool":"cig","from":"1.20.0","to":"1.20.1"}]}]}` |
| `carton show` | `{"carton":"0.1.0","tools":[{"tool":"cig","build":"1.20.0","trust":"pinned","targets":["linux-x86_64"]}],"packs":{"standard":["cig","cigo"]}}` |
| `update` | as `carton <v>` plus `{"checked":true,"installed":false}` under `--check`; with `--exit-code` the exit is 10 when newer |
| `which` | `{"tool":"cigo","build":"1.17.0","carton":"0.1.0","path":"/home/u/.cigscript/toolchains/cigo/1.17.0/bin/cigo","why":"pinned in .cig-carton","steps":[{"step":2,"result":"hit"}]}` |
| `list` | `{"cartons":["0.1.0"],"sticks":[{"tool":"cigo","build":"1.16.0"}],"disk_bytes":123}` |
| `check` | `{"findings":[{"code":"E1801","severity":"refuse","headline":"...","detail":[],"fixes":["..."],"subject":"cigo"}]}` |
| `doctor` | `{"sections":[{"name":"shims","findings":[{"i_think":"...","because":"...","fix":"..."}]}]}` |
| `repair` | `{"restored":[{"tool":"cig","version":"1.20.0","file":"bin/cig","from":"store"}],"failed":[]}` |
| `uninstall`, `remove carton`, `gc` | `{"removed":["<path>"],"freed_bytes":0,"dry_run":false}` |
| `shims list` | `{"shims":[{"name":"cig","target":"...","first_on_path":true}]}` |
| `config get\|set\|list` | `{"config":{"channel":"stable"}}` |
| `history` | `{"entries":[{"at":"...","what":"update","from":"0.1.0","to":"0.1.1"}]}` |
| `explain` | the registry node: `{"code","title","meaning","fix","no","arises","tags","since","related","causes":[...]}` |
| `bench` | `{"shim_dispatch_ns":240000,"check_ms":0,"switch_ms":0,"shim_bytes":344008,"installer_bytes":0}` |

Progress, prompts and notices are never printed in JSON mode. A question that would be asked takes its safe default (exit 3 when that default was a refusal) and nothing is written to stderr.
