What `cig run --dry-run --plain --no-color --no-doctor <script>` printed, recorded from the
released cig 1.20.0 (linux-x86_64) with a fresh CIGSCRIPT_HOME and XDG_CACHE_HOME, stdin closed.
Each script was run twice, a second apart: stdout, stderr and the exit code were identical
every time. The plan is on stderr, the script's own output on stdout, so the trial compares both.
A dry run printed no version string, no run id and no timestamp, and wrote nothing into
CIGSCRIPT_HOME. The normalizer still masks those (and the scratch path) so a later cig that does
print them does not make every script look different.
