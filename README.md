# cig-installer

`ciginstaller` (short: `cigi`) installs, pins, updates, repairs and removes
the [CigScript](https://github.com/otmof-ops/cig-script) family: `cig`,
`cigo`, `cigcompile` and `cigar`, at the versions a project asks for.

**Status: being built.** Nothing here installs anything yet. The plan of
record is the issues, starting with the architecture
([#1](https://github.com/otmof-ops/cig-installer/issues/1)): one binary,
versioned toolchains in `~/.cigscript/`, and shims that run the version each
project pins.

## Licensing

- **Code**: [Apache-2.0](LICENSE), the same license as CigScript. In plain
  words: use, copy, change, sell and embed it freely, including in closed
  products; keep the copyright notice and the [NOTICE](NOTICE) file with any
  copy; you get a patent license from every contributor; and you can't use the
  name to endorse your derivative.
- **Documentation**: [CC BY 4.0](LICENSES/CC-BY-4.0.txt). Reuse and adapt
  with credit.
- **The name.** ciginstaller is a trademark of Jay Taylor, as CigScript is;
  [TRADEMARKS.md](TRADEMARKS.md) says what is free to do and what to ask about
  first.

[REUSE.toml](REUSE.toml) maps every path to its license. Contributions are
accepted under the same terms, signed off under the DCO; see
[CONTRIBUTING.md](CONTRIBUTING.md). Security reports:
[SECURITY.md](SECURITY.md).
