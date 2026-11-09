# Security policy

## Supported versions

Fixes go into the newest code on `main` until ciginstaller has releases; after
that, into the newest release.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository (**Security
→ Report a vulnerability**). Do not open a public issue for a security
problem. Include the commit or version, the platform, and the command that
shows the problem. You'll get an acknowledgement within a few days and a fix
or a mitigation before anything is disclosed.

## What ciginstaller promises

An installer decides what runs on your machine, so it is held to more than
the rest of the family:

- **Nothing runs that was not verified.** Every build it installs is checked
  against the digest its signed manifest names before it is unpacked or run.
  A way to make it install or run anything else is a security bug.
- **An install is a burn.** It is journaled and can be rolled back, and a
  failed install leaves the machine as it was.
- **No telemetry, ever.** It reports nothing about you or your machine to
  anyone unless you send a report yourself.
