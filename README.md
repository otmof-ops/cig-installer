# cig-installer

`ciginstaller` (short: `cigi`) installs, pins, updates, repairs and removes
the [CigScript](https://github.com/otmof-ops/cig-script) family: `cig`,
`cigo`, `cigcompile` and `cigar`, at the versions a project asks for.

**Status: being built.** Nothing here installs anything yet. The foundation
is in: the carton manifest and the checks that say whether a combination fits,
the store and the journal that make every change one atomic, undoable step,
verified downloads, the shim that runs the version a project pins, and the
error pages (`ciginstaller explain`). Installing comes next. The plan of
record is the issues, starting with the architecture
([#1](https://github.com/otmof-ops/cig-installer/issues/1)) and its decision,
[docs/decisions/0001-architecture.md](docs/decisions/0001-architecture.md).

## Licensing

ciginstaller is **source-available, not open source.** The code is under the
[Functional Source License, version 1.1, with an Apache-2.0 future license
(FSL-1.1-ALv2)](LICENSE). The Open Source Initiative does not count it as an
open source license, and this project does not call it one.

- **What you may do.** Use it, including inside a company for its own work;
  read it, study it, change it and share it; and consult with it, meaning
  help others use it. The source is here for all of that.
- **What you may not do.** Sell it, or offer it in a product or service that
  competes with it.
- **It becomes Apache-2.0.** Each version converts to
  [Apache-2.0](LICENSES/Apache-2.0.txt) two years after it is made available,
  and from then on that version carries Apache-2.0 terms.
- **Earlier commits.** The first 31 commits were published under Apache-2.0
  before this license took over. [LICENSING.md](LICENSING.md) lists which
  versions carry which license.
- **Documentation**: [CC BY 4.0](LICENSES/CC-BY-4.0.txt). Reuse and adapt
  with credit.
- **The name.** ciginstaller is a trademark of Jay Taylor, as CigScript is;
  [TRADEMARKS.md](TRADEMARKS.md) says what is free to do and what to ask about
  first.

This is a summary. The [LICENSE](LICENSE) file governs, and it is the text to
read before you rely on anything here. [REUSE.toml](REUSE.toml) maps every path
to its license. Contributions are accepted under the contributor agreement; see
[CONTRIBUTING.md](CONTRIBUTING.md). Security reports:
[SECURITY.md](SECURITY.md).
