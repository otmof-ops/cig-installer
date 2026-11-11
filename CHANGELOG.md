<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# Changelog

All notable changes to cig-installer. The format follows Keep a Changelog; the
project follows semantic versioning. Nothing is released yet, so everything
below is under Unreleased, and the first version will take it as its section.

### A bug you reported is told to you when a carton fixes it, for every tool (#12)

- After an update, a report this machine filed against cigo, cigcompile, cigar or the installer
  whose fix the new carton carries is said once: "the bug you reported (cig-orchestrator#12,
  <title>) is fixed in cigo 1.18.0, which this update installed with carton 0.2.0". cig's own
  reports stay cig's (`cig crash announce-fixed`).
- The daily notice that a newer carton is out also says which of your reported bugs it fixes, in
  the issue's words: "... is fixed in cigo 1.4.2, which is in carton 1.1.3. update: ciginstaller
  update". Each report is told once: `announced` and `fixed_in` are written into it.
- The fixed version comes from the manifest's advisories: one whose `url` is the issue's, or whose
  `id` is `owner/repo#N` or `repo#N`, with `fixed_in`. `cargo xtask manifest assemble` writes them
  from each new build's release notes (`Fixes #12`, `closes owner/repo#7`, GitHub's closing
  keywords), with no `affects`, so they reach the reporter and nobody else.

### Shims know their home, and a moved home never runs the old one (#46, #47)

- A shim finds its home from where it sits (`<home>/bin/<name>`); `CIGSCRIPT_HOME` still overrides, and `CIGI_SHIM_VERBOSE=1` and `doctor` name both when they differ. The shim passes the home it found to the installer and the tool, so `export PATH=<home>/bin:$PATH` is the whole of the advice and cigo finds cig through the same home. `ciginstaller` run from `<home>/bin` works on that home too.
- E1840 names the home it searched and the shim's own folder, and no longer says "missing" when `ciginstaller` is beside the shim.
- The shim index is `cigi-index 2`: paths relative to the home, each with the `(dev, inode, size, mtime)` its receipt recorded. The older absolute form is still read.
- New E1902: the shim refuses a target that resolves outside its home. New E1903: it refuses a target that no longer matches its receipt (one `stat` per run, no hashing). `ciginstaller repair` checks the files against their receipts, records them again and rewrites the index; `doctor` names "moved from <old home>".

## Unreleased

Installing, pinning, switching, updating and rolling back the family, on
Linux, with every build checked first and every change one undoable step.

### The humidor is the shared store (#1)

- The store moved from `store/` to `humidor/store/`, so one shelf holds every version of every tool and the objects behind them. cig's index and its `humidor/<ver>/cig` links are untouched, and cig skips the `store/` directory because it is not a version.
- A home made by the old layout is migrated once, at the first command that writes, as the journaled transaction `migrate store`: the objects directory is renamed (nothing is copied, hard links stay), and an interruption or a failure restores the old home byte for byte. Commands that only read use either layout.
- `gc`, `repair`, `doctor` and `uninstall --all` follow the new layout; `uninstall --all` removes `humidor/` only when nothing of anyone else's is in it.
- ADR 0001 is accepted. Section 3 is rewritten for the shared store, and the decision on reusing cig's crates is recorded as it stands: parity tests hold it, and the extraction waits on cig-compile#1.

### Quick closes (#49, #11, #48)

- `update --no-trial` says `trial: skipped (--no-trial)` once, in the voice, plain and `--json` forms (#49).
- A report another tool queued (cig, cigo, cigcompile, cigar) carries the installer's version block when it is shown or sent, marked as taken at send time; `crash show` and `crash send` make the same object (#11).
- Plain `http://` mirrors are refused (E1900, the installer's second range, E1900-E1999) unless they are on a loopback address, and over loopback only builds a pin vouches for are accepted, whatever `allow_single_source` says. `config set mirror` refuses a non-loopback `http://`, and `doctor` reports one already set (#48).
- Certificates are checked against the system's trust store, which honors `SSL_CERT_FILE` and `SSL_CERT_DIR`, and the bundled Mozilla roots only on a system with none; an unpinned build over loopback http is refused before anything is asked for. Tests run their mirror over HTTPS with a throwaway certificate, and `https_to_http_redirect_refused` now redirects for real (#48).

### One doctor for the whole family (#13, all but the TUF root line)

- **`ciginstaller doctor`** now includes each installed tool's own doctor (`<tool> __doctor --json`,
  60 s limit each, cigcompile included); **`--tools`** shows only those.
- **The guard's kernel features:** the kernel against the guard's minimum (5.12, which cig's contained box needs), user namespaces
  (`max_user_namespaces`) and `bpf` among the active security modules, each read-only and each
  ending with its one fix command. They are notes, never actions. `CIGI_DOCTOR_PROBE_ROOT` points
  the probes at a folder for tests and changes nothing but where doctor reads from.
- **The tin's disk use** beside the store, with `cig tin gc` as the fix (a note above 1 GiB).

### Performance budgets on a release build, on every platform (#20)

- **The `bench` workflow** (called by `ci.yml` on pull requests and on `main`) builds
  release and runs `ciginstaller bench --strict` against a fixture carton on Linux and
  macOS (`ci/bench-release.sh`), then the budget tests at their real limits. A row the
  job must measure that was not measured fails it; nothing "not measured" counts as
  passing.
- **Windows** runs the rows that need no shell-script product (shim dispatch, binary
  sizes, the atomic switch, disk for five cigo versions); every other row is printed as
  not measured.
- **`crates/cigi-budgets`** runs the budget tests (`budget_20`, `budget`, `a3_shim_perf`,
  a new `release_install`, and `budget_20_neutral`, which has no unix-only fixtures) on a
  release build. They could not be built in release from `ciginstaller`, whose
  fault-injection test feature refuses to compile there on purpose. `a3_shim_perf` now
  asserts the 2 ms budget itself on a release build (5 ms headroom on a debug build).
- `bench` names the line that says what failed when an install it times fails, not the
  `note:` before it.
- README: the budget table says where each row is measured.

### Signed update channel (#9)

- **`TufTrust`** in `cigi-source` admits the manifest only through TUF metadata: a root compiled
  into the binary, then timestamp, snapshot, targets and the delegated `releases` role, each
  checked for signature threshold, expiry and version rollback, plus a compiled-in version floor,
  and the manifest's length and SHA-256 checked against its signed target. A build is admitted at
  the new `signed` trust level against its signed entry and attestation reference; a compiled-in pin
  that disagrees is a refusal. `--offline` waives only the timestamp, snapshot and release expiries,
  says so, and never the root's or targets'.
- **Releases need no offline key.** The offline `targets` key signs a delegation to an online
  `releases` role once; a release is signed by the online releases, snapshot and timestamp keys.
  Decision 0004 is accepted, with the delegation and its trade-off written into it.
- **`cargo xtask tuf keygen|init|sign|rotate|floor`** writes and signs the metadata with keys read
  from files outside the repository (a path inside it is refused), and **`manifest assemble
  --tuf-entries`** emits the signed-target entries. The attack tests (rollback, freeze,
  mix-and-match, a bad mirror, an expired timestamp, a threshold not met, rotations, a delegated
  key outside its paths, stolen online keys) run in the workspace suite against `TufTrust`.
- **A `tuf` workflow** signs the online roles daily and on request from secrets
  (`TUF_RELEASES_KEY`, `TUF_SNAPSHOT_KEY`, `TUF_TIMESTAMP_KEY`) and publishes to the `tuf` release,
  timestamp last. The compiled-in root is a placeholder until the key ceremony.

### Compiled programs (#30, the unblocked part)

- **`ciginstaller programs [--project DIR] [--json]`** lists the compiled programs
  on the machine: name, where, the cig it was built for, the cigcompile that made
  it, and any advisory of the cached manifest that covers that cig, with its
  severity and fixed-in version. They are found by their trailer (the layout cig
  documents, read by `cig-compat` with no dependency on cig) on `PATH` and in the
  project's folders and their `bin/`; the disk is never scanned. A damaged
  trailer is skipped with a note.
- **`ciginstaller programs rebuild <name>`** rebuilds a program from its script
  with the carton's cigcompile (run only from a build that matches its receipt,
  with a time limit) on the installed cig, checks it with `cigcompile verify` and
  `cigcompile test`, and swaps it in as one transaction; the old program stays
  until the new one has passed. With no script on the machine it says so and
  names the advisory. `--dry-run` builds nothing. E1859.
- **`status` and `doctor`** say once per program when it is built on a cig an
  advisory covers; `status --json` carries `waiting.programs` only then.
- Still blocked: the signed compatibility index ([#9](https://github.com/otmof-ops/cig-installer/issues/9)).

### Crash reports (#10, #28 part 1)

- **`ciginstaller crash list|show|send|delete|submit`** over cig's own ashtray
  (`<home>/ashtray`, the same files and fields), so every tool's crash is in
  one queue. `show` prints exactly what `send` sends; `send` honors `reports`
  (`never` refuses, also as `CIG_REPORTS=never`) and carries no identifier.
  With no issues route yet (the relay and token of #28, the courier of #24)
  the report stays queued and the output says so. E1857.
- **The installer's own crash** is written to the ashtray in cig's schema with
  the origin `ciginstaller`; nothing is sent.
- **`schemas/report.schema.json`** is the allowlist for every product, and the
  family fingerprint (kind, location, message, product, major version) never
  includes a path, user name or host. The family's own paths stay readable in
  titles and bodies.

### Issue routes, blame and the machine block (#28 steps 2 and 4)

- **Routes that prove themselves.** `crash send` files a report through `gh`
  (your login) or the API with `CIG_GITHUB_TOKEN`, searching the blamed
  repository first so a known fault joins its issue and a lost response never
  files it twice. With neither, `--via link` gives a prefilled issue and
  `--via file --out FILE` writes it out. GitHub's own error text is shown,
  never hidden. The relay and a scoped token wait for the courier (#24).
- **`crash test-route`** makes one dry search per route. **`crash status`**
  shows the queue, the `reports` setting and each route's last success and
  failure, and `doctor` shows the same.
- **Blame routing**: a report is filed in the repository of the product at
  fault (the top frame that names one, then the error code's range, then the
  origin), with a line in the origin's issue; a moved issue is followed.
- **The `cig-report v1` block** ends every issue body: fingerprint, kind,
  blame, version matrix, repro and its verification, and the first bad carton
  when known; labels name the kind, area, regression and severity.
- **A fake tracker** in `cigi-testkit` speaks GitHub's issue and search API,
  with 422s, 403s, rate limits and redirects, and every route is tested
  against it.
- The report schema accepts the kinds `differential` and `update`, and
  `crash show` and `crash list` name the blamed repository as the target.

### Reports (#11, #3)

- **`ciginstaller report [<run>]`** puts together what a bug report needs, and
  writes or sends nothing until you say so (`--out FILE`, `--send`).
  - **The version block**: the installer, the carton and channel, each tool's
    build (its version, the digest of what was installed, how it was
    trusted), what the project pins (as versions and file names, never
    paths), the OS and kernel, the metadata versions, and whether the
    combination is inside the manifest's guarantees. A combination no carton
    holds is flagged by name, as in "cigo 1.18.0 with cig 1.20.0: not from the
    same carton", and so is a build whose `requires` the running builds do
    not meet or that the manifest yanks.
  - **The bundle**: the block, the operation's journal (redacted), the trial's
    differences (`update` now keeps them with its transaction) and the
    metadata versions it saw. For a cig run, cig's own bundle through
    `cig report-bundle <run> --json`, or `cig --json report <run>` for a cig
    that does not have it yet.
  - **`--block --json`** is the machine form of the block that a product
    embeds in its own report.
  - **`--send`** goes through the crash route and the `reports` setting
    (`never` refuses), as a report of kind `report`, with no identifier.
  - It works when `state.json` is the broken thing: the block says so.
  - E1858 when the bundle cannot be built or written.

### Bug reports (#29, step 1)

- **`ciginstaller bug`** writes up what went wrong, in the person's words, with
  the evidence for it, and shows exactly what would travel before anything is
  saved. `bug --run <id>`, `bug <product>` (or `--product`), `bug --last`, and
  `bug list` / `bug show <n>` for the reports saved here (the receipt).
  - **The conversation**: which part, what you were doing, what you expected,
    what happened, which run; in the voice with plain beside it, every
    question skippable. "Not sure" takes the part from the run or its error
    code (E1000-E1099 CigCompile and programs, E870-E899 cigo's guard,
    E1800-E1899 the installer, any other cig). Without a terminal nothing is
    asked: `--part`, `--did`, `--expected`, `--happened`, `--every-time`.
  - **The words stay yours.** A span shaped like a path, an address, a token,
    an e-mail address or your own name is marked and asked about, and is never
    rewritten without being listed. A token-shaped span is always replaced.
  - **The evidence**: the run's bundle through `report`'s cig call, the version
    matrix, the carton and channel, the OS and `doctor`'s findings for that
    product. A verified repro in the run's bundle goes in as `verified on this
    machine`; without one the report says so and asks whether it happens every
    time. A report is never held back for want of a repro.
  - **The preview is the saved object**: `route::travel` of the one report that
    is saved, section by section. Every section can be taken out
    (`--without`) except the version matrix.
  - **Nothing without a yes**: the last question, or `--save` (kept here) or
    `--send` (through `crash send`'s route and consent; `reports = never`
    refuses). `--yes` answers none of it, and no request is made before a yes.
  - E1858 when the report cannot be put together.
- Not yet: the similarity search and "add mine", `bug status` and
  `bug withdraw`, the evidence for cigo, the guard, CigCompile and updates, and
  each product's own short way in.

### Tripwires (#28, the installer's row)

- **After an update or a rollback the installer reads its work back from
  disk**: the shim-index against `state.json`, every shim against the carton's
  build, and after a rollback every toolchain against its receipt. A mismatch
  keeps the work (nothing is undone and the exit stays 0), prints
  `warning E1849` with the next step (`ciginstaller repair`, then
  `ciginstaller crash show <report>`), and saves a report of kind `tripwire`
  in the ashtray: high severity after a rollback, medium after an update.
  Nothing is sent.

### Release prune and the archive index (#26)

- **`cargo xtask release prune --dry-run`** lists, per product, which
  releases stay (the three newest, and every version a carton or a `--pin`
  names, with who pins it) and which would move to the archive, with a reason
  for each. Moving them (`--yes`) waits for the signed index (#9) and refuses
  for now.
- **`archive/index.json`** (format 1), read by `cig-compat`: where each
  archived version's tag and assets went, with their digests. A manifest names
  it with `[archive] index = "https://..."`.
- **Downloads fall back to the archive**: when a build's release is gone from
  the product's repository, the installer reads the index the manifest names
  and downloads the archived copy, still at the digest the manifest pins. An
  index that lists other bytes for a build is refused (E1825), and the index
  is only read when the release is gone.

### The machine layer and locked settings (#17)

- **An organization's settings sit below ciginstaller, read only.** Linux and the BSDs read `/etc/cigscript/config.toml`; macOS reads the managed preferences a configuration profile installs, `/Library/Managed Preferences/io.github.otmof-ops.cigscript.plist`; Windows reads `HKLM\SOFTWARE\Policies\otmof-ops\CigScript`. Each lists its locks (`locked = [...]`, a `Locked` array, a `Locked` REG_MULTI_SZ). `docs/ORGANIZATIONS.md` has the formats.
- **A locked value beats the environment, the person's file and a flag.** An unlocked machine value sits below the person's file and above the built-in default. `config get`, `config list` and `status --json` say `locked by <store>`; `status --json` carries a `locks` object for an organization's own tools.
- **`config set` and `config unset` on a locked key refuse with new E1904,** which names the store and writes nothing. An install answer (`--preset`, `--reports`, `--answers`) for a locked setting is E1904 too; the locked value counts as the answer.
- **Five settings an organization locks:** a pinned carton (the new `carton` setting, below a project's `.cig-carton` when unlocked and above everything, a script's `# cig` header and `CIGI_CARTON` included, when locked), `auto_update`, `reports` (a locked `never` stops `bug --send`, `report --send` and every `crash send` whatever `CIG_REPORTS` says), `mirror`, and the guard `preset` (recorded and locked here; enforcement is cig-orchestrator#66's).
- **A store that is there and cannot be read is an error that names it** (E1830), never skipped: an unknown setting, a bad value, a lock that names nothing, a file anyone can write, a file root does not own. A machine with no store behaves as before.
- **The shim sends a run down the slow path when the machine layer mentions `carton`,** so a locked carton also holds on the fast path.
- No variable, file or flag moves the machine layer in a release build; tests reach a stand-in folder through a feature release builds do not have.

### Staged rollouts (#17)

- **`rollout = N%`** (1 to 100, default 100): a share of machines takes a new
  carton on its own first, so a bad carton is caught on a few machines. The
  share is decided on the machine, by a hash of the machine id with a fixed
  prefix reduced to 0-99, and neither the id nor the hash is ever printed,
  stored or sent. Outside the share the update notice says the carton is
  rolling out; `ciginstaller update` still takes it, because you asked, and
  says so. A machine with no id, or a `rollout` value that is not a percent,
  waits below 100.
- **`status --json`** gains `cartons` and `rollout` (`percent`, `in_share`,
  `reason`) for an organization's own inventory tools. It makes no request.

### CigarScript in the carton's tools (#1, #3)

- **`[tool.cigar]`** in `manifest/tools.toml`, as CigarScript's release
  workflow names its builds (`cigar-v{version}-{os}-{arch}{exe}` with a
  `.sha256`, five targets, attested), with the `cigar` shim, and
  `cargo xtask manifest assemble --cigar <version>`. No carton names it until
  CigarScript has a release. CigarScript ships one binary, so there is no
  `cigar-lsp` shim yet.

### What ciginstaller gives CIG.OS (#22, first slice)

- **The questions as data:** `install --dry-run --json` lists every question
  an install would ask (the text in the voice and in plain, the choices and
  the setting each writes, the default), and asks, writes and fetches
  nothing.
- **Answers handed in:** `--answers FILE.json`, `--preset` and `--reports`,
  so a welcome screen asks the questions itself and the install runs with no
  terminal. An answer that is not a choice is E1870 before anything changes.
- **Asked once:** answered questions are recorded in `state.json` and not
  asked again; `status --json` carries `first_run`.
- **Preseeding:** `install --yes carton <c> --from <bundle.tar>` with the
  answers installs with no network and no question, as an image build does.
- Waiting on their own issues: the preset's enforcement
  (cig-orchestrator#66), the guard daemon at level 5 (cig-orchestrator#57),
  the offsite-backup question (cig-orchestrator#65) and Secure Boot (#23).

### Licenses (#27)

- **`ciginstaller licences [tool] [--text] [--json]`** lists the installer and
  every installed build with its license (an SPDX id, or "not stated"), the
  SPDX page, where the license file is, and what was acknowledged. `--text` prints
  one license file. It reads this machine only and sends nothing.
- **A per-build `license` field** (an SPDX expression, optional) in the
  manifest's `[product.*]` tables, read by `cig-compat`, filled by
  `cargo xtask manifest assemble` from `manifest/products.toml` (which says
  so when a build has none), and recorded in each receipt at install. A
  manifest or receipt without it still works and shows "not stated".
- **The first-install display of the licenses**: shown once, and again only
  when a license new to the machine comes in. The licenses are copyright
  permissions, so the notice is acknowledged, not accepted: the answer
  (carton, the installer license's sha256, time, how) is kept in `state.json`
  and sent nowhere. `--yes` records `acknowledged-by-flag`; with no terminal
  the licenses are noted in the report, not confirmed; a no, Ctrl-C or end of
  input at a terminal installs nothing (E1876). E1877 and E1878 are the
  `licences` errors.

### Bootstrap script

- **`install.sh` and `install.ps1` templates** (`packaging/bootstrap/`) and
  `ci/fill-bootstrap.sh`, which writes a release's tarball digests into them.
  The script downloads its platform's tarball from that release over HTTPS,
  checks the SHA-256 (and `gh attestation verify` where `gh` exists), refuses
  on any mismatch with nothing left behind, unpacks under `CIGSCRIPT_HOME` and
  runs `ciginstaller install` with the arguments it was given. Integrity, not
  authorship: nothing is signed until #9. The release workflow attaches the
  filled scripts, with checksums and attestations, to the draft release. There
  is no installer release yet. `install.ps1` stops until a Windows build exists.
- `ci/test-bootstrap.sh` runs the filled script against a local fixture in CI.

### License

- **The code is under the Functional Source License, version 1.1, with an
  Apache-2.0 future license (FSL-1.1-ALv2)** from the licensing commit on
  (#27). Each version converts to Apache-2.0 on the second anniversary of the
  date it is made available. FSL is source-available, not open source.
- The first 31 commits were published under Apache-2.0 before this change, are
  tagged `apache-2.0-last`, and stay under it; copies taken then keep that
  grant. [LICENSING.md](LICENSING.md) lists which versions carry which license.
- Documentation is CC-BY-4.0. [TRADEMARKS.md](TRADEMARKS.md), `NOTICE`,
  `THIRD-PARTY-NOTICES.md` and `REUSE.toml` say the rest, and contributions
  come in under the contributor agreement.

### Offline: bundle, mirror and install from a file (#16)

- **`bundle --carton <v> [--targets a,b] --out <file.tar>`** writes one file
  holding the manifest and its checksum, every build of the carton for those
  targets with its checksum, and a small index. Only builds admitted at the
  usual levels go in, and the bytes are checked again as they are written. The
  same inputs give the same file.
- **`install --from <file.tar>`** unpacks the bundle into a private directory in
  the home, installs from it with no network, and removes it. A bundle is one
  source, so it is `pinned` or refused exactly as a mirror directory is, and
  nothing lowers the floor. Bundles are unsigned until #9, and the output says
  so. The doctor names installs that came from one source, a bundle by name.
- **`mirror <dir>`** fills a directory with the manifest and every build, each
  admitted, laid out for `install --from <dir>` and `mirror = ...`. Running it
  again only adds, repairs a damaged file, and removes nothing. The manifest
  goes in last, and a manifest older than the one already there is refused.
- New codes: E1837 (a bundle that cannot be unpacked), E1838 (a mirror
  directory that is refused), E1839 (a bundle that cannot be written).

### Documentation

- A README in the family's order, `SECURITY.md` that says what is verified and
  what is not, `SUPPORT.md`, and this changelog.
- `docs/ERRORS.md`, generated from `errors/registry.toml` by
  `cargo xtask errors-doc`, with a test that fails when it is stale.
- A logo and a social preview in the family's style.

### The foundation (#36)

- **The carton manifest** and the checks that say whether a combination fits
  (the marry-up matrix, #4): a product that needs a newer `cig`, a stick
  outside its carton, a yanked build, an advisory.
- **The home.** One directory (`~/.cigscript`, or `--home`, or
  `CIGSCRIPT_HOME`), the installer's own content-addressed store, receipts
  for every installed file, and a lock.
- **Transactions.** Every change is journaled and ends in one atomic switch
  of the shim index. A kill before the switch rolls back and a kill after it
  rolls forward, and the next run recovers either.
- **Verified downloads.** One HTTPS client with no identifiers, a manifest
  admitted before anything it names, and three levels for a build: `pinned`,
  `attested` where `gh` can verify, and `checksummed` where two sources agree.
- **The shim** that runs the version a project pins, with a median of 0.24 ms
  from start to `exec` on the shim's own clock.
- **Output.** One grammar (a headline, the detail, the next step) in the
  voice and in `plain`, truecolor with 256 and 16-color fallbacks, `NO_COLOR`,
  and `--json` with one object per result ([docs/JSON.md](docs/JSON.md)).
- **Errors.** A registry of codes `E1800` to `E1899`, each with its kind of
  no, its ranked causes and the command to type next, and
  `ciginstaller explain <code>`.
- **No telemetry** as a tested property (#19): the HTTP client and the
  fixture server check that requests carry no identifier, no cookie and no
  query string.

### Install, pin and resolve (#37, #38)

- **`install carton`, `install pack <name>`, `install pack --pick a,b` and
  `install stick <tool>`**, each from one carton, as one transaction, with
  the PATH block shown before it is written (`--no-path` skips it).
- **`carton`, `carton list`, `carton show`, `channel`, `use`, `list`.**
- **`pin carton <pin>`, `pin cig <pin>` and `which <tool> [--why]`.** A
  project pins a carton in `.cig-carton`, or a key in `cig-tasks.json` or
  `cigar.toml`; the resolver says which step chose the build.
- **`cargo xtask manifest assemble` and `cargo xtask pins`**, which assemble
  the next manifest from the products' own releases and write the compiled-in
  pins.

### Update, roll back and tidy (#39)

- **`update`:** check, choose, verify, trial and switch, in that order. The
  trial runs the new `cig` against the project's scripts and shows what it
  plans differently. `--check` only looks, and `--exit-code` exits 10 when a
  newer carton exists.
- **`rollback [tool]`** to the previous carton, or one tool's previous build.
- **`self update`**, **`gc`** and **`check`**.
- **The release workflow:** a `v*` tag builds ciginstaller and its shim for
  Linux x86_64 and aarch64, with a checksum and a build attestation for each,
  and creates a draft release. Publishing it is a human act.

### Bench and the budgets (#35, #20, #19)

- **`bench`** measures the installer's own budgets on this machine, locally and
  read-only, and compares them with `bench/bench.toml`: shim dispatch, `check`,
  the switch (in a scratch home), the sizes, the first lines, and, with
  `--from <mirror>`, the installs. Each shows its median and p95. `--json` has
  the reserved shape, extended. `--strict` fails with `E1866` when one is over.
  The family view, cartons against cartons, `--watch` and the window's Bench view
  are not built: they wait on otmof-ops/cig-script#179, #30, #33 and #29, and
  say so with `E1867`.
- **`bench_after_update`** (off by default): after an update, a short local sample
  against the last one, with a regression shown once. Nothing is sent anywhere.
- **Budget tests** for `check`, the switch, a whole carton, five cigo versions on
  disk, the update check and the binary sizes; the no-telemetry test now covers
  `update`, `doctor --network`, `repair`, `uninstall` and `self update --check`.
