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
| `bundle` | `{"out":"/abs/carton-0.1.0.tar","carton":"0.1.0","channel":"stable","sequence":3,"targets":["linux-x86_64"],"builds":2,"files":[{"name":"manifest-stable.toml","size":1234,"sha256":"..."}],"bytes":123456,"signed":false}` |
| `mirror` | `{"dir":"/abs/mirror","channel":"stable","sequence":3,"cartons":["0.1.0"],"targets":["linux-x86_64"],"added":["cig-v1.20.0-linux-x86_64"],"kept":0,"bytes_added":123,"manifest_written":true,"left_out_yanked":[]}`; a re-run has `"added":[]` and `"manifest_written":false` |
| `install` | `{"carton":"0.1.0","tools":{"cig":"1.20.0"},"trust":{"pinned":1,"attested":0,"checksummed":2},"path_block":"added"\|"present"\|"skipped","txn":"<id>","dry_run":false,"source":"github"\|"dir:/path"\|"mirror:https://..."\|"bundle:file.tar"}` |
| `carton` | `{"carton":"0.1.0","newer":"0.1.1"\|null,"channel":"stable"}` |
| `carton <v>`, `use`, `rollback` | `{"from":"0.1.0","to":"0.1.1","trial":{"scripts":14,"same":14,"differ":0}\|null}` |
| `carton list` | `{"channel":"stable","cartons":[{"version":"0.1.1","installed":true,"yanked":false,"changes":[{"tool":"cig","from":"1.20.0","to":"1.20.1"}]}]}` |
| `carton show` | `{"carton":"0.1.0","tools":[{"tool":"cig","build":"1.20.0","trust":"pinned","targets":["linux-x86_64"]}],"packs":{"standard":["cig","cigo"]}}` |
| `update` | as `carton <v>` plus `{"checked":true,"installed":false}` under `--check`; with `--exit-code` the exit is 10 when newer |
| `which <tool>` | `{"tool":"cigo","build":"1.17.0","carton":"0.1.0","path":"/home/u/.cigscript/toolchains/cigo/1.17.0/bin/cigo","why":"pinned in .cig-carton","steps":[{"step":2,"result":"hit"}]}` |
| `which cig --script <file>` | the `which <tool>` shape, plus `"script":{"file":"/home/u/work/s.cig","header":">=1.6.2"}` when the script carries a `# cig` pin; `steps` then holds the `H0` to `H3` lines for the header after the numbered ones |
| `which` (no tool) | `{"tools":[{"tool":"cig","version":"1.20.0","carton":"0.1.0","path":"...","why":"...","steps":[...]}]}`; a tool that does not resolve has `"version":null`, `"code"` and `"error"` |
| `list` | `{"cartons":["0.1.0"],"sticks":[{"tool":"cigo","build":"1.16.0"}],"disk_bytes":123}` |
| `licences` | `{"installer":{"tool":"ciginstaller","version":"0.1.0","license":"FSL-1.1-ALv2","spdx_urls":["https://spdx.org/licenses/FSL-1.1-ALv2.html"],"text_sha256":"...","repository":"https://github.com/otmof-ops/cig-installer"},"builds":[{"tool":"cig","version":"1.20.0","from":["carton 0.1.0"],"license":"Apache-2.0"\|null,"spdx_urls":[],"text_path":"/abs/LICENSE"\|null,"source":{"repo":"otmof-ops/cig-script","tag":"v1.20.0"}\|null}],"terms":{"carton":"0.1.0","licenses":["Apache-2.0","FSL-1.1-ALv2"],"how":"acknowledged"\|"acknowledged-by-flag"\|"acknowledged-without-terminal","at":"...","installer_license_sha256":"..."}\|null}`; `license` is the code license (product docs are CC-BY-4.0 and are not named); it is `null` when the manifest stated none ("not stated" on the page); with a tool named, `installer` is `null` unless it is `ciginstaller` |
| `install` (terms) | `install` gains `"terms"`: the `terms` object above when this install showed the licenses, else `null`; it is also written to `state.json` under `terms` |
| `check` | `{"findings":[{"code":"E1801","severity":"refuse","headline":"...","detail":[],"fixes":["..."],"subject":"cigo"}]}` |
| `doctor` | `{"needs_action":1,"notes":0,"findings":[{"section":"shims","severity":"ok"\|"note"\|"action","found":"...","because":"...","fix":"ciginstaller repair"\|null,"code":"E1845"\|null}],"tools":[{"tool":"cig","ok":true,"output":"...","code":null}]}`; `tools` is empty without `--tools` |
| `repair` | `{"changed":true,"restored":[{"tool":"cig","version":"1.20.0","path":"bin/cig","from":"store"\|"download"}],"removed":[{"tool":"cig","version":"1.20.0","path":"bin/extra"}]}`; under `--dry-run`, `{"dry_run":true,"from_store":1,"downloads":0}` |
| `uninstall <tool>` | `{"tool":"cigo","versions":["1.17.0"],"removed":[...]}` |
| `uninstall --all` | `{"changed":true,"removed":[{"path":"bin/cig","kind":"link"\|"toolchain"\|"file"\|"object","sha256":"...","bytes":0}],"notes":["..."],"journal":"<path>"\|null}`; under `--dry-run`, `{"dry_run":true,"would_remove":[...]}` |
| `remove <carton>` | `{"carton":"0.1.0","removed":[...]}` |
| `crash` (list) | `{"reports":[{"id":"20261110T141515Z-927346","origin":"ciginstaller","kind":"crash","version":"0.1.0","state":"local"\|"pending"\|"filed"\|"failed: <why>","title":"...","fingerprint":"92734611","target":"otmof-ops/cig-installer","issue":"https://..."\|null,"attempts":0,"last_error":null}],"waiting":1,"unreadable":0,"reports_setting":"ask"\|"always"\|"never","route":{"configured":false,"host":null}}`; every tool's reports, newest first |
| `crash show <id>` | `{"id":"...","state":"local","target":"otmof-ops/cig-installer","report":{...}}`; `report` is exactly the object `crash send` sends, whose keys are the properties of `schemas/report.schema.json` |
| `crash send <id>` / `--all` | `{"sent":[{"id":"...","url":"https://..."\|null}],"already_filed":[],"queued":[]}`; with no issues route `{"sent":[],"queued":["<id>"],"route":{"configured":false}}` and exit 0; `reports = never` is E1857 with exit 2, a route that fails is E1857 with exit 1, and the report stays queued either way |
| `crash delete <id>` / `--all` | `{"deleted":["<id>"]}`; `--all` needs `--yes-irreversible` |
| `crash submit --origin <tool> --file <f>` | `{"id":"...","sent":false,"origin":"cig-orchestrator","fingerprint":"..."}`; with `--now` the `send` shape |
| `gc` | `{"removed":["<path>"],"freed_bytes":0,"dry_run":false}` |
| `shims sync` | `{"shims":["cig","cigo"],"relinked":[],"removed":[],"index_rewritten":false}` |
| `shims list` | `{"shims":[{"name":"cig","target":"..."\|null,"error":null,"present":true,"first_on_path":true}]}` |
| `config list` | `{"config":{"channel":"stable"},"settings":[{"key":"channel","value":"stable","source":"environment"\|"file"\|"default","default":"stable","allowed":["stable","beta","nightly"]}]}` |
| `config get <key>` | `{"config":{"channel":"stable"},"key":"channel","value":"stable","source":"file","default":"stable"}` |
| `config set <key> <value>` | `{"config":{"channel":"beta"},"key":"channel","value":"beta","previous":"stable","changed":true}` |
| `config unset <key>` | `{"config":{"channel":"stable"},"key":"channel","value":"stable","previous":"beta","source":"default"}` |
| `history` | `{"entries":[{"id":"<id>","at":"...","what":"update","from":"0.1.0","to":"0.1.1","ok":true,"txn":"<id>"}]}`, newest first |
| `explain` | the registry node: `{"code","title","meaning","fix","no","arises","tags","since","related","causes":[...]}` |
| `bench` | `{"shim_dispatch_ns":240000,"check_ms":12.4,"switch_ms":8.1,"shim_bytes":344008,"installer_bytes":6170000,` then, added: `"shim_dispatch_p95_ns","check_p95_ms","switch_p95_ms","disk_five_cigo_ratio","first_line_status_ms","first_line_list_ms","install_pack_dev_ms","install_carton_ms","build":"debug"\|"release","runs":100,"over_budget":0,"budgets":[{"id","what","source","unit","limit","median","p95","runs","status":"ok"\|"over"\|"info"\|"not measured","note"}],"not_measured_by_bench":[{"id","what","limit","unit"}],"not_built":[{"view","blocked_by"}]}`; a figure that was not measured is `null`. The five reserved keys keep their meaning; times are medians. `--strict` with a budget over is an error, `E1866`. |

Progress, prompts and notices are never printed in JSON mode. A question that would be asked takes its safe default (exit 3 when that default was a refusal) and nothing is written to stderr.

## A crash in the installer

A panic writes a report to the ashtray (origin `ciginstaller`) and, under `--json`, prints one line on stdout and nothing on stderr: `{"ok":false,"command":"status","error":{"code":"E1890","kind":"internal","message":"...","hint":"ciginstaller crash show <id>","subject":"<fingerprint>","report":"~/d1/f2.json"}}`. Nothing is sent.
