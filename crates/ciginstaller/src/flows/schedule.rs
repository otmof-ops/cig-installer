// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The background schedule of `auto_update = security | all` (#7): one job a day that runs
//! `ciginstaller update --auto`, at most daily, on mains power, when the machine is idle, with
//! jitter, so a fleet does not call the release host at the same second.
//!
//! - Linux: a systemd user timer and service (`cigi-update.timer`, `cigi-update.service` in
//!   `~/.config/systemd/user`). `ConditionACPower=true`, idle CPU and I/O scheduling classes and
//!   `RandomizedDelaySec=2h`. Enabled with `systemctl --user daemon-reload` and
//!   `systemctl --user enable --now cigi-update.timer`.
//! - macOS: a launchd agent (`~/Library/LaunchAgents/io.github.otmof-ops.cigi-update.plist`),
//!   `StartInterval` of a day plus a per-machine offset, `ProcessType Background`,
//!   `LowPriorityIO`. launchd has no power condition, so `update --auto` asks `pmset` itself.
//!   Enabled with `launchctl bootstrap gui/<uid> <plist>`.
//! - Windows: a Task Scheduler task, its definition in `installer/schedule/` of the home (UTF-16
//!   XML): daily with a random delay, `DisallowStartIfOnBatteries`, `RunOnlyIfIdle`, the user's own
//!   token and no elevation. Enabled with `schtasks /Create /TN <name> /XML <file> /F`.
//!
//! Every file goes through the installer's journal and is recorded in `state.json` (`schedule`),
//! so an interrupted write is undone, `auto_update = off` (or `notify`) takes it away again, and
//! `uninstall --all` removes it. The calls that turn it on and off go through [`Enabler`]: the
//! installer's own [`OsEnabler`], or, when `CIGI_SCHEDULE_ENABLER=record` in a fault-injection test build (never a release build), a
//! [`RecordEnabler`] that writes the command it would have run to
//! `installer/schedule-enable.log` and runs nothing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cigi_home::{Home, State, Txn};
use cigi_ui::InstallerError;
use serde_json::json;

use crate::cx::Cx;
use crate::errors::ie;
use crate::flows::settle;

/// The key in `state.json` that records what was written.
pub const KEY: &str = "schedule";
pub const SYSTEMD_UNIT: &str = "cigi-update";
pub const LAUNCHD_LABEL: &str = "io.github.otmof-ops.cigi-update";
pub const WINDOWS_TASK: &str = "ciginstaller-auto-update";
/// The longest a schedule's random delay may be.
pub const JITTER_SECS: u64 = 2 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    pub fn host() -> Os {
        match std::env::consts::OS {
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            _ => Os::Linux,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }
}

/// What a plan is made from.
#[derive(Debug, Clone)]
pub struct Spec {
    pub os: Os,
    /// `<home>/bin/ciginstaller`: the link that survives a self-update.
    pub exe: PathBuf,
    pub home_root: PathBuf,
    pub user_home: PathBuf,
    pub installer_dir: PathBuf,
    /// `XDG_CONFIG_HOME` when it is set to an absolute path.
    pub xdg_config: Option<PathBuf>,
}

/// The files and the calls of a schedule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub files: Vec<(PathBuf, Vec<u8>)>,
    /// Run after the files are written, in order. `{uid}` stands for the user's numeric id.
    pub enable: Vec<Vec<String>>,
    /// Run before the files are removed.
    pub disable: Vec<Vec<String>>,
    /// Run after the files are removed.
    pub forget: Vec<Vec<String>>,
}

/// A per-machine offset in seconds, `0..JITTER_SECS`, from the home's path (FNV-1a): the same on
/// every run, different between machines.
pub fn jitter(home_root: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in home_root.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h % JITTER_SECS
}

fn argv(root: &Path) -> Vec<String> {
    vec![
        "--home".into(),
        root.to_string_lossy().into_owned(),
        "--yes".into(),
        "update".into(),
        "--auto".into(),
    ]
}

/// A word of a systemd `ExecStart=` line: quoted, with `\`, `"`, `%` and `$` escaped.
fn systemd_word(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '%' => o.push_str("%%"),
            '$' => o.push_str("$$"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn utf16(text: &str) -> Vec<u8> {
    let mut b = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b
}

pub fn plan(s: &Spec) -> Plan {
    let args = argv(&s.home_root);
    match s.os {
        Os::Linux => {
            let dir = s
                .xdg_config
                .clone()
                .unwrap_or_else(|| s.user_home.join(".config"))
                .join("systemd")
                .join("user");
            let exec = std::iter::once(s.exe.to_string_lossy().into_owned())
                .chain(args)
                .map(|w| systemd_word(&w))
                .collect::<Vec<_>>()
                .join(" ");
            let service = format!(
                "[Unit]\nDescription=ciginstaller: install the carton auto_update allows\n\
                 ConditionACPower=true\n\n[Service]\nType=oneshot\nExecStart={exec}\n\
                 Nice=19\nIOSchedulingClass=idle\nCPUSchedulingPolicy=idle\n"
            );
            let timer = format!(
                "[Unit]\nDescription=ciginstaller: daily check for the carton auto_update allows\n\n\
                 [Timer]\nOnCalendar=daily\nRandomizedDelaySec={JITTER_SECS}\nPersistent=true\n\
                 AccuracySec=10min\nUnit={SYSTEMD_UNIT}.service\n\n[Install]\nWantedBy=timers.target\n"
            );
            let sc = |a: &[&str]| -> Vec<String> {
                ["systemctl", "--user"]
                    .iter()
                    .chain(a.iter())
                    .map(|x| x.to_string())
                    .collect()
            };
            Plan {
                files: vec![
                    (
                        dir.join(format!("{SYSTEMD_UNIT}.service")),
                        service.into_bytes(),
                    ),
                    (
                        dir.join(format!("{SYSTEMD_UNIT}.timer")),
                        timer.into_bytes(),
                    ),
                ],
                enable: vec![
                    sc(&["daemon-reload"]),
                    sc(&["enable", "--now", &format!("{SYSTEMD_UNIT}.timer")]),
                ],
                disable: vec![sc(&["disable", "--now", &format!("{SYSTEMD_UNIT}.timer")])],
                forget: vec![sc(&["daemon-reload"])],
            }
        }
        Os::Macos => {
            let path = s
                .user_home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist"));
            let items: String = std::iter::once(s.exe.to_string_lossy().into_owned())
                .chain(args)
                .map(|w| format!("    <string>{}</string>\n", xml(&w)))
                .collect();
            let every = 86_400 + jitter(&s.home_root);
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                 <plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{LAUNCHD_LABEL}</string>\n\
                 \x20 <key>ProgramArguments</key>\n  <array>\n{items}  </array>\n\
                 \x20 <key>StartInterval</key>\n  <integer>{every}</integer>\n\
                 \x20 <key>RunAtLoad</key>\n  <false/>\n\
                 \x20 <key>ProcessType</key>\n  <string>Background</string>\n\
                 \x20 <key>LowPriorityIO</key>\n  <true/>\n  <key>Nice</key>\n  <integer>19</integer>\n\
                 </dict>\n</plist>\n"
            );
            let target = format!("gui/{{uid}}/{LAUNCHD_LABEL}");
            Plan {
                enable: vec![vec![
                    "launchctl".into(),
                    "bootstrap".into(),
                    "gui/{uid}".into(),
                    path.to_string_lossy().into_owned(),
                ]],
                disable: vec![vec!["launchctl".into(), "bootout".into(), target]],
                forget: Vec::new(),
                files: vec![(path, text.into_bytes())],
            }
        }
        Os::Windows => {
            let path = s
                .installer_dir
                .join("schedule")
                .join("auto-update-task.xml");
            let at = 3 * 3600 + jitter(&s.home_root);
            let start = format!("2026-01-01T{:02}:{:02}:00", at / 3600, (at % 3600) / 60);
            let arguments: Vec<String> = args
                .iter()
                .map(|a| {
                    if a.contains(' ') {
                        format!("\"{a}\"")
                    } else {
                        a.clone()
                    }
                })
                .collect();
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n\
                 <Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\r\n\
                 \x20 <RegistrationInfo><Description>ciginstaller: install the carton auto_update allows</Description></RegistrationInfo>\r\n\
                 \x20 <Triggers><CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>\
                 <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>\
                 <RandomDelay>PT{}M</RandomDelay></CalendarTrigger></Triggers>\r\n\
                 \x20 <Principals><Principal id=\"Author\"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\r\n\
                 \x20 <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\
                 <DisallowStartIfOnBatteries>true</DisallowStartIfOnBatteries>\
                 <StopIfGoingOnBatteries>true</StopIfGoingOnBatteries>\
                 <StartWhenAvailable>true</StartWhenAvailable>\
                 <RunOnlyIfIdle>true</RunOnlyIfIdle>\
                 <IdleSettings><Duration>PT10M</Duration><WaitTimeout>PT1H</WaitTimeout><StopOnIdleEnd>true</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>\
                 <ExecutionTimeLimit>PT1H</ExecutionTimeLimit><Priority>7</Priority></Settings>\r\n\
                 \x20 <Actions Context=\"Author\"><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions>\r\n\
                 </Task>\r\n",
                JITTER_SECS / 60,
                xml(&s.exe.to_string_lossy()),
                xml(&arguments.join(" ")),
            );
            Plan {
                enable: vec![vec![
                    "schtasks".into(),
                    "/Create".into(),
                    "/TN".into(),
                    WINDOWS_TASK.into(),
                    "/XML".into(),
                    path.to_string_lossy().into_owned(),
                    "/F".into(),
                ]],
                disable: vec![vec![
                    "schtasks".into(),
                    "/Delete".into(),
                    "/TN".into(),
                    WINDOWS_TASK.into(),
                    "/F".into(),
                ]],
                forget: Vec::new(),
                files: vec![(path, utf16(&text))],
            }
        }
    }
}

// ------------------------------------------------------------------------------ the seam

/// The one place a schedule is turned on or off in the operating system.
pub trait Enabler {
    fn run(&self, argv: &[String]) -> Result<(), String>;
}

/// The installer's own: runs the command (`systemctl --user ...`, `launchctl ...`, `schtasks ...`).
pub struct OsEnabler;

impl Enabler for OsEnabler {
    fn run(&self, argv: &[String]) -> Result<(), String> {
        let Some((prog, rest)) = argv.split_first() else {
            return Ok(());
        };
        let rest: Vec<String> = rest
            .iter()
            .map(|a| a.replace("{uid}", &user_id()))
            .collect();
        let mut child = Command::new(prog)
            .args(&rest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{prog} did not start: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match child.try_wait() {
                Ok(Some(st)) if st.success() => return Ok(()),
                Ok(Some(st)) => return Err(format!("{prog} exited with {st}")),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{prog} did not finish in 30s"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(format!("{prog} could not be waited for: {e}")),
            }
        }
    }
}

fn user_id() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".to_string())
}

/// Writes each command it is given to a log and runs nothing.
pub struct RecordEnabler {
    pub log: PathBuf,
}

impl Enabler for RecordEnabler {
    fn run(&self, argv: &[String]) -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_err(|e| e.to_string())?;
        writeln!(f, "{}", argv.join(" ")).map_err(|e| e.to_string())
    }
}

pub fn enabler(home: &Home) -> Box<dyn Enabler> {
    if cigi_home::schedule_enabler_recorded() {
        Box::new(RecordEnabler {
            log: home.installer().join("schedule-enable.log"),
        })
    } else {
        Box::new(OsEnabler)
    }
}

// -------------------------------------------------------------------------- the record

/// What `state.json` says was written: the files, and the OS they are for.
pub fn recorded(state: &State) -> Option<(Os, Vec<PathBuf>)> {
    let v = state.extra.get(KEY)?;
    let os = match v.get("os")?.as_str()? {
        "macos" => Os::Macos,
        "windows" => Os::Windows,
        _ => Os::Linux,
    };
    let files = v
        .get("files")?
        .as_array()?
        .iter()
        .filter_map(|f| f.as_str().map(PathBuf::from))
        .collect();
    Some((os, files))
}

pub fn spec_for(home: &Home) -> Spec {
    let os = Os::host();
    Spec {
        os,
        exe: home.bin().join(if os == Os::Windows {
            "ciginstaller.exe"
        } else {
            "ciginstaller"
        }),
        home_root: home.root(),
        user_home: home.user_home(),
        installer_dir: home.installer(),
        xdg_config: std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute()),
    }
}

/// Does `auto_update` ask for a schedule?
pub fn wanted(cx: &Cx) -> bool {
    matches!(cx.cfg.get("auto_update").as_str(), "security" | "all")
        && cx.cfg.get("update_check") != "off"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Unchanged,
    Written(Vec<PathBuf>),
    Removed(Vec<PathBuf>),
}

fn in_place(plan: &Plan) -> bool {
    plan.files
        .iter()
        .all(|(p, b)| std::fs::read(p).is_ok_and(|have| have == *b))
}

fn delete_file(txn: &mut Txn, home: &Home, p: &Path) -> Result<(), InstallerError> {
    if std::fs::symlink_metadata(p).is_err() {
        return Ok(());
    }
    if p.starts_with(home.root()) {
        txn.delete(p).map_err(ie)
    } else {
        txn.delete_user_file(p).map_err(ie)
    }
}

/// Bring the schedule in line with `want`: write and enable it, take it down, or do nothing.
/// Idempotent. The files go through one journaled transaction with the record in `state.json`;
/// the enabling calls run after it commits, and a call that fails is a note, not a failure (the
/// files are there, and the next command tries again).
pub fn apply(
    cx: &mut Cx,
    want: bool,
    spec: &Spec,
    enabler: &dyn Enabler,
    notes: &mut Vec<String>,
) -> Result<Change, InstallerError> {
    let have = recorded(&cx.state);
    let p = plan(spec);
    if want {
        if have.is_some() && in_place(&p) {
            return Ok(Change::Unchanged);
        }
        cx.lock_and_recover()?;
        let mut txn = cx.begin("schedule: write the update schedule")?;
        let written = (|| -> Result<State, InstallerError> {
            for (path, bytes) in &p.files {
                if let Some(dir) = path.parent() {
                    txn.mkdir_all(dir).map_err(ie)?;
                }
                txn.write(path, bytes, Some(0o644)).map_err(ie)?;
            }
            let mut st = cx.state.clone();
            st.extra.insert(
                KEY.to_string(),
                json!({
                    "os": spec.os.name(),
                    "files": p.files.iter().map(|(f, _)| f.to_string_lossy()).collect::<Vec<_>>(),
                }),
            );
            st.save_in(&mut txn).map_err(ie)?;
            Ok(st)
        })();
        let st = settle(txn, written)?;
        cx.state = st;
        for c in &p.enable {
            if let Err(e) = enabler.run(c) {
                notes.push(format!(
                    "the schedule is written but `{}` failed: {e}; run it yourself, or the next command tries again",
                    c.join(" ")
                ));
            }
        }
        return Ok(Change::Written(
            p.files.iter().map(|(f, _)| f.clone()).collect(),
        ));
    }
    let Some((_, files)) = have else {
        return Ok(Change::Unchanged);
    };
    cx.lock_and_recover()?;
    for c in &p.disable {
        if let Err(e) = enabler.run(c) {
            notes.push(format!("`{}` failed: {e}", c.join(" ")));
        }
    }
    let mut txn = cx.begin("schedule: remove the update schedule")?;
    let home = cx.home.clone();
    let removed = (|| -> Result<State, InstallerError> {
        for f in &files {
            delete_file(&mut txn, &home, f)?;
        }
        let mut st = cx.state.clone();
        st.extra.remove(KEY);
        st.save_in(&mut txn).map_err(ie)?;
        Ok(st)
    })();
    cx.state = settle(txn, removed)?;
    for c in &p.forget {
        if let Err(e) = enabler.run(c) {
            notes.push(format!("`{}` failed: {e}", c.join(" ")));
        }
    }
    Ok(Change::Removed(files))
}

/// After a command that went well: make the schedule match `auto_update` (a machine-locked value
/// counts like any other). Quiet when nothing changes; one line when it does.
pub fn reconcile(cx: &mut Cx) {
    let want = wanted(cx);
    if !want && !cx.state.extra.contains_key(KEY) {
        return;
    }
    let spec = spec_for(&cx.home);
    let en = enabler(&cx.home);
    let mut notes = Vec::new();
    let line = match apply(cx, want, &spec, en.as_ref(), &mut notes) {
        Ok(Change::Unchanged) => None,
        Ok(Change::Written(f)) => Some(format!(
            "auto_update = {}: the daily update schedule is in place ({})",
            cx.cfg.get("auto_update"),
            f.first()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )),
        Ok(Change::Removed(_)) => Some("the daily update schedule was taken away".to_string()),
        Err(e) => Some(format!(
            "the update schedule could not be changed: {}",
            e.message
        )),
    };
    if let Some(l) = line {
        cx.ui.notice(&l);
    }
    for n in notes {
        cx.ui.notice(&n);
    }
}

/// For `uninstall --all`: take the schedule down in the system (before anything is removed) and
/// give back the recorded files, which the uninstall deletes in its own transaction.
pub fn take_down(home: &Home, state: &State) -> (Vec<PathBuf>, Vec<String>) {
    let Some((os, files)) = recorded(state) else {
        return (Vec::new(), Vec::new());
    };
    let spec = Spec {
        os,
        ..spec_for(home)
    };
    let en = enabler(home);
    let mut notes = Vec::new();
    for c in &plan(&spec).disable {
        if let Err(e) = en.run(c) {
            notes.push(format!(
                "`{}` failed: {e}; turn the update schedule off by hand",
                c.join(" ")
            ));
        }
    }
    (files, notes)
}

/// Delete the recorded schedule files inside an open transaction (`uninstall --all`).
pub fn delete_recorded(
    txn: &mut Txn,
    home: &Home,
    files: &[PathBuf],
) -> Result<(), InstallerError> {
    for f in files {
        delete_file(txn, home, f)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(os: Os) -> Spec {
        Spec {
            os,
            exe: PathBuf::from("/h/.cigscript/bin/ciginstaller"),
            home_root: PathBuf::from("/h/.cigscript"),
            user_home: PathBuf::from("/h"),
            installer_dir: PathBuf::from("/h/.cigscript/installer"),
            xdg_config: None,
        }
    }

    fn text(p: &Plan, i: usize) -> String {
        String::from_utf8(p.files[i].1.clone()).unwrap()
    }

    #[test]
    fn linux_writes_a_user_timer_with_ac_idle_and_jitter() {
        let p = plan(&spec(Os::Linux));
        assert_eq!(
            p.files[0].0,
            PathBuf::from("/h/.config/systemd/user/cigi-update.service")
        );
        assert_eq!(
            p.files[1].0,
            PathBuf::from("/h/.config/systemd/user/cigi-update.timer")
        );
        let (service, timer) = (text(&p, 0), text(&p, 1));
        assert!(service.contains("ConditionACPower=true"), "{service}");
        assert!(
            service.contains("IOSchedulingClass=idle")
                && service.contains("CPUSchedulingPolicy=idle")
        );
        assert!(
            service.contains(
                "ExecStart=\"/h/.cigscript/bin/ciginstaller\" \"--home\" \"/h/.cigscript\" \"--yes\" \"update\" \"--auto\""
            ),
            "{service}"
        );
        assert!(timer.contains("OnCalendar=daily") && timer.contains("RandomizedDelaySec=7200"));
        assert!(timer.contains("Persistent=true") && timer.contains("WantedBy=timers.target"));
        assert_eq!(
            p.enable,
            vec![
                vec!["systemctl", "--user", "daemon-reload"],
                vec![
                    "systemctl",
                    "--user",
                    "enable",
                    "--now",
                    "cigi-update.timer"
                ],
            ]
        );
        assert_eq!(p.disable[0][2], "disable");
    }

    #[test]
    fn xdg_config_home_and_odd_characters_are_respected() {
        let mut s = spec(Os::Linux);
        s.xdg_config = Some(PathBuf::from("/x/cfg"));
        s.exe = PathBuf::from("/h/my \"home\"/100%/bin/ciginstaller");
        let p = plan(&s);
        assert!(p.files[0].0.starts_with("/x/cfg/systemd/user"));
        assert!(
            text(&p, 0).contains("\"/h/my \\\"home\\\"/100%%/bin/ciginstaller\""),
            "{}",
            text(&p, 0)
        );
    }

    #[test]
    fn macos_writes_a_launch_agent_with_a_day_and_an_offset() {
        let p = plan(&spec(Os::Macos));
        assert_eq!(
            p.files[0].0,
            PathBuf::from("/h/Library/LaunchAgents/io.github.otmof-ops.cigi-update.plist")
        );
        let t = text(&p, 0);
        assert!(
            t.contains("<string>io.github.otmof-ops.cigi-update</string>"),
            "{t}"
        );
        assert!(t.contains("<string>--auto</string>") && t.contains("<string>Background</string>"));
        assert!(t.contains("<key>LowPriorityIO</key>"));
        let every = 86_400 + jitter(Path::new("/h/.cigscript"));
        assert!(t.contains(&format!("<integer>{every}</integer>")), "{t}");
        assert_eq!(p.enable[0][..3], ["launchctl", "bootstrap", "gui/{uid}"]);
        assert_eq!(p.disable[0][1], "bootout");
    }

    #[test]
    fn windows_writes_a_task_with_battery_and_idle_conditions_in_utf16() {
        let p = plan(&spec(Os::Windows));
        assert_eq!(
            p.files[0].0,
            PathBuf::from("/h/.cigscript/installer/schedule/auto-update-task.xml")
        );
        let b = &p.files[0].1;
        assert_eq!(&b[..2], &[0xFF, 0xFE], "a UTF-16 LE byte order mark");
        let units: Vec<u16> = b[2..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let t = String::from_utf16(&units).unwrap();
        for want in [
            "<DisallowStartIfOnBatteries>true",
            "<RunOnlyIfIdle>true",
            "<RandomDelay>PT120M</RandomDelay>",
            "<DaysInterval>1</DaysInterval>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<Command>/h/.cigscript/bin/ciginstaller</Command>",
            "--yes update --auto",
        ] {
            assert!(t.contains(want), "{want} in {t}");
        }
        assert_eq!(
            p.enable[0][..4],
            ["schtasks", "/Create", "/TN", "ciginstaller-auto-update"]
        );
        assert_eq!(p.disable[0][1], "/Delete");
    }

    #[test]
    fn the_jitter_is_stable_and_within_two_hours() {
        let a = jitter(Path::new("/home/a/.cigscript"));
        assert_eq!(a, jitter(Path::new("/home/a/.cigscript")));
        assert!(a < JITTER_SECS);
        assert_ne!(a, jitter(Path::new("/home/b/.cigscript")));
    }

    #[test]
    fn the_recording_enabler_writes_the_command_and_runs_nothing() {
        let d = tempfile::tempdir().unwrap();
        let e = RecordEnabler {
            log: d.path().join("log"),
        };
        e.run(&["systemctl".into(), "--user".into(), "daemon-reload".into()])
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("log")).unwrap(),
            "systemctl --user daemon-reload\n"
        );
    }
}
