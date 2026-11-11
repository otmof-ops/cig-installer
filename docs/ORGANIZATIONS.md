<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Organizations: the machine layer and locked settings

An organization that manages many machines sets ciginstaller's defaults, and locks the ones it
needs held, in one place per system. ciginstaller reads that place and never writes it: the
person using the machine cannot change a locked setting, and an administrator changes it where
it is kept.

Nothing here sends anything anywhere. `ciginstaller status --json` is what an organization's own
tools collect, and it makes no request.

## Where the machine layer lives

| system | the store | the locks |
|---|---|---|
| Linux and the BSDs | `/etc/cigscript/config.toml` | `locked = ["carton", ...]` |
| macOS | the managed preferences a configuration profile installs, `/Library/Managed Preferences/io.github.otmof-ops.cigscript.plist` | a `Locked` array of strings |
| Windows | `HKLM\SOFTWARE\Policies\otmof-ops\CigScript` | a `Locked` value of type `REG_MULTI_SZ` |

The macOS preference domain is the bundle identifier `io.github.otmof-ops.cigscript`; a
configuration profile with a Custom Settings payload for that domain installs the file. Property
lists in XML and in binary form are both read (a binary one through the system's own `plutil`).

A setting is written as its name, with the value ciginstaller's own file would take. On Windows a
text setting is `REG_SZ` (or `REG_EXPAND_SZ`, never expanded) and a number is `REG_DWORD`; value
names do not care about case there.

### An example for Linux

```toml
# /etc/cigscript/config.toml, owned by root, mode 0644
carton = "1.1"
auto_update = "security"
reports = "never"
mirror = "https://mirror.example.com/cig/"
preset = "strict"
keep_days = 30

locked = ["carton", "auto_update", "reports", "mirror", "preset"]
```

`keep_days` is set and not locked: it is a default the person may change.

### What the layer may hold

- Any setting `ciginstaller config` lists can be set, and any of them can be locked. The five an
  organization asks for most are below.
- A setting is checked the way `config set` checks it. A lock that names something that is not
  a setting is an error, so a typo is never a lock that quietly does nothing.
- A lock with no value locks the setting at its built-in default: nobody can change it, and
  nothing else sets it. For `carton` that means no carton is pinned by the machine.

## Which value wins

From strongest to weakest:

1. a machine value that is locked;
2. the environment (`CIGI_<SETTING>`) and a flag for this run;
3. the person's own `installer/config`;
4. a machine value that is not locked;
5. the built-in default.

A locked value cannot be overridden by the person: not by the environment, not by their file, not
by a flag. Setting a locked key with `config set` or `config unset` is refused with `E1904`,
which names the store, and nothing is written. An install answer for a locked setting
(`--preset`, `--reports`, an `--answers` file) is refused the same way; when nothing is asked, a
locked setting counts as already answered, so the first-run questions leave it out.

`ciginstaller config get <setting>`, `config list` and `status --json` say where a value comes
from, `locked by <store>` for a locked one.

## The five settings

### A pinned carton

`carton` takes the same pin a project's `.cig-carton` holds: `1`, `1.1` or `1.1.4`. A project's pin
still means what it meant ([#6](https://github.com/otmof-ops/cig-installer/issues/6)): the project
picks the newest installed carton that matches, and its builds of every product.

- **Locked,** the carton wins over everything: a project's `.cig-carton` and its older per-tool
  pins, a script's `# cig >=...` header, and `CIGI_CARTON`. `ciginstaller which cig` says
  `carton locked by <store>`, and the resolution trail begins with a line that says the lock is in
  force. `ciginstaller pin carton` still writes a project's file, and says the lock wins on this
  machine. A pinned carton that is not installed is fetched under the usual `pin_fetch` policy,
  verified like any other.
- **Not locked,** `carton` is a pin for the whole machine, from the machine layer or from the
  person's own file. It sits below a project's pins and above "the machine's carton", so a project
  can still ask for its own.

When the machine layer mentions `carton`, the shim sends every run to the installer, where the
lock is applied, instead of choosing a build on its own fast path.

### Automatic updates

`auto_update` is `off`, `notify`, `security` or `all`. Locked to `off`, the update notice stays
quiet, and the person's own `ciginstaller update` still works.

### Reports

`reports` is `ask`, `always` or `never`. Locked to `never`, every way of sending a report refuses
with `E1857`, which says the machine locks it: `ciginstaller bug --send`, `ciginstaller report
--send` and `ciginstaller crash send` (one report, `--all`, `--via link` and `--via file`
included). `CIG_REPORTS=always` in the person's shell does not change that. Reports are still
written to the machine's own queue, where the person can read them. A `reports` locked to `ask`
or `always` is the machine's word in the same way.

### A mirror

`mirror` is a place to fetch the family from instead of GitHub. It must be `https://`, a
`file://` address or a path (plain `http://` only to a loopback address, as always). Locked, the
person's `CIGI_MIRROR` and their own file do not change it, and `ciginstaller doctor` checks it.

### The guard preset

`preset` is `chill`, `balanced`, `strict` or `paranoid`. It is recorded and locked here. What a
preset switches on is cigo's, and waits for
[cig-orchestrator#66](https://github.com/otmof-ops/cig-orchestrator/issues/66), which will read
the locked value; until then `ciginstaller config get preset` says so.

## A store that cannot be read

A store that is there and cannot be read is an error that names it (`E1830`), and no command goes
on without it: a lock that cannot be read is not a lock that is absent. The same goes for a value
that is not valid, a setting or lock that is not known, a file larger than 256 KiB, and, on Unix,
a file that anyone but its owner can write or that root does not own (the system's own file).
A machine with no store at all behaves as if the layer did not exist.

## Staged rollouts and the inventory

`rollout = N%` is the share of machines that take a new carton on their own, chosen by a hash of
the machine id that never leaves the machine. It can be set in the machine layer like any other
setting. `ciginstaller status --json` reports each machine's standing and its locks:

```json
{"rollout": {"percent": 10, "in_share": false, "reason": null},
 "locks": {"source": "/etc/cigscript/config.toml",
           "locked": [{"key": "carton", "value": "1.1", "locked_by": "/etc/cigscript/config.toml"}]}}
```

## What is not here

- Per-user managed preferences on macOS (`/Library/Managed Preferences/<user>/`) and
  `HKEY_CURRENT_USER` policies are not read: the layer is the machine's.
- Locks do not stop a person who owns the machine from replacing the program itself. They keep a
  setting from being changed by a command, a variable or a file that is not the organization's.
