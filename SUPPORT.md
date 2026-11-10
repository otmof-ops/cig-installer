<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# Getting help

I read every request. Put yours where it fits and you'll get an answer in
the issue.

| you want to | open |
|---|---|
| get unstuck installing, pinning or updating | [an issue](https://github.com/otmof-ops/cig-installer/issues/new), and say what you were trying to do |
| report something that went wrong | [an issue](https://github.com/otmof-ops/cig-installer/issues/new), with the command and everything it printed |
| ask for a platform, a way in or a command it doesn't have | [an issue](https://github.com/otmof-ops/cig-installer/issues/new), and say what you do today instead |
| report a problem in the language itself | [CigScript's issues](https://github.com/otmof-ops/cig-script/issues) |
| report a security problem | [SECURITY.md](SECURITY.md), never a public issue |

## What helps

- `ciginstaller --version` and your platform.
- The command you ran and everything it printed, including the code on the
  `error[E18xx ...]` line. Paste, don't summarize.
- `ciginstaller explain <code>` has the long page for that code, with its
  likely causes in order. [docs/ERRORS.md](docs/ERRORS.md) has all of them.
- `ciginstaller check --json` for a combination that will not fit, and
  `ciginstaller which <tool> --why` when the wrong version runs.
- Run it again with `--home <a scratch directory>` if you can. That shows
  whether the problem is in the installer or in what is already on your machine.

## What happens next

- **Questions:** a direct answer, with the steps for your machine, in the
  issue. If the answer should be in the docs, it goes in the docs too.
- **Bugs:** a fix, and a test that keeps it fixed. The issue is closed with
  the version that has it.
- **Requests:** a reply saying what I think and why. Accepted ones are built
  with you, and the plan of record is the issues.

## Why this page exists

cig-installer is the front door to the family, and a front door that sticks
is the worst place to give up. If something here is confusing, that's a bug in
the docs, and I'd rather hear about it than have you leave.
