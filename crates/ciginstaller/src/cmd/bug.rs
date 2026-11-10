// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `bug` (#29 step 1): the one way a person reports anything in the family.
//!
//! On a terminal it asks, in the voice with plain beside it, what part, what you were doing, what
//! you expected, what happened and which run, and every question can be skipped. Without one
//! (`--json`, CI, a pipe) nothing is asked: the answers come from flags and anything missing stays
//! empty. The person's words stay theirs: a span shaped like a path, an address, a token or an
//! e-mail address is marked and asked about, and is never rewritten without being listed.
//!
//! Then the preview: exactly the object `crash show` prints and `crash send` sends, section by
//! section, before anything is saved. A report is saved only on a yes (the last question, or
//! `--save`), and sent only through the crash route and its consent (`--send`, or "send it").
//! `--yes` answers none of these questions. No request is made before a yes.
//!
//! `bug list` and `bug show <n>` are the receipt: the reports saved on this machine.

use std::path::PathBuf;

use cigi_ui::{Answer, Choice, How, Icon, InstallerError, Mode, Question, Report, Ui, EXIT_USAGE};
use serde_json::{json, Value};

use super::crash as crash_cmd;
use crate::cli::{BugCmd, BugSub};
use crate::crash::{self, anon, Report as Saved};
use crate::cx::{Cx, Globals};
use crate::flows::bug as flow;

use cigi_ui::catalogue::Msg;

const OPENER: Msg = Msg {
    id: "bug.opener",
    voice: "something went wrong. let's write it up properly. every question can be skipped.",
    plain:
        "writing a bug report. every question can be skipped; an empty line skips a text question.",
};
const Q_PART: Msg = Msg {
    id: "bug.part",
    voice: "1. which part?",
    plain: "1. which part of the family is it about?",
};
const Q_DID: Msg = Msg {
    id: "bug.did",
    voice: "2. what were you doing?",
    plain: "2. what were you doing when it went wrong?",
};
const Q_EXPECTED: Msg = Msg {
    id: "bug.expected",
    voice: "3. what did you expect?",
    plain: "3. what did you expect to happen?",
};
const Q_HAPPENED: Msg = Msg {
    id: "bug.happened",
    voice: "4. what happened instead?",
    plain: "4. what happened instead?",
};
const Q_RUN: Msg = Msg {
    id: "bug.run",
    voice: "5. which run?",
    plain: "5. which run is it about?",
};
const Q_ANOTHER: Msg = Msg {
    id: "bug.another-run",
    voice: "the run's id:",
    plain: "the id of the run:",
};
const Q_EVERY: Msg = Msg {
    id: "bug.every-time",
    voice: "does it happen every time?",
    plain: "does it happen every time you do this?",
};
const Q_MARK: Msg = Msg {
    id: "bug.mark",
    voice: "that looks like {0}: {1}. keep it as you typed it, or swap in {2}?",
    plain: "\"{1}\" looks like {0}. keep it as typed, or replace it with \"{2}\"?",
};
const SAID_TOKEN: Msg = Msg {
    id: "bug.token",
    voice: "{1} looks like a secret, so it goes: it is {2} in the report",
    plain: "\"{1}\" looks like a secret and is always replaced: it is \"{2}\" in the report",
};
const Q_SUGGEST: Msg = Msg {
    id: "bug.suggest",
    voice: "that looks like {0} to me ({1}). go with that?",
    plain: "{1} points to {0}. use it?",
};
const Q_FINAL: Msg = Msg {
    id: "bug.final",
    voice: "that is exactly what would travel. what now?",
    plain: "this is exactly what would be sent. what now?",
};
const Q_SECTION: Msg = Msg {
    id: "bug.section",
    voice: "which section comes out? answers, environment, doctor, run or repro (the version matrix stays):",
    plain: "which section should be removed? answers, environment, doctor, run or repro (the version matrix stays):",
};

fn usage(msg: impl Into<String>) -> InstallerError {
    InstallerError::new("E1870", msg).with_exit(EXIT_USAGE)
}

/// Can a person be asked? A terminal in and out, and not `--json`.
fn can_ask(ui: &Ui) -> bool {
    ui.mode != Mode::Json && ui.tty_in && ui.tty_err
}

/// The index of the choice taken; an answer nobody gave stops the command with nothing saved.
fn asked(ui: &Ui, q: &Question) -> Result<usize, InstallerError> {
    let a: Answer = ui.ask(q);
    if a.how == How::Aborted {
        return Err(usage("no answer was given: nothing was saved or sent"));
    }
    Ok(a.index)
}

fn question(text: String, choices: Vec<Choice>, default: usize) -> Question {
    Question {
        text,
        summary: None,
        detail: Vec::new(),
        choices,
        default,
        irreversible: false,
    }
}

// ----- the receipt ------------------------------------------------------------------------

/// The bug reports saved here, newest first.
/// The product named by `--product`, or by the positional the family's messages use.
fn product_of(c: &BugCmd) -> Option<&str> {
    c.product.as_deref().or(c.product_arg.as_deref())
}

fn mine(env: &crash_cmd::Env) -> Result<Vec<Saved>, InstallerError> {
    let l =
        crash::list(&env.root).map_err(|e| usage(format!("the ashtray cannot be read: {e}")))?;
    Ok(l.reports.into_iter().filter(flow::is_bug).collect())
}

fn part_of_saved(r: &Saved) -> String {
    r.details
        .as_ref()
        .and_then(|d| d["part"].as_str())
        .unwrap_or("unsure")
        .to_string()
}

fn sections_of(r: &Saved) -> Vec<&'static str> {
    let d = r.details.clone().unwrap_or(Value::Null);
    let mut v = Vec::new();
    for (key, word) in [
        ("answers", "answers"),
        ("version_matrix", "version-matrix"),
        ("environment", "environment"),
        ("doctor", "doctor"),
        ("run", "run"),
    ] {
        if d.get(key).is_some() {
            v.push(word);
        }
    }
    if r.repro.is_some() {
        v.push("repro");
    }
    v
}

fn list(env: &crash_cmd::Env) -> Result<Report, InstallerError> {
    let all = mine(env)?;
    let rows: Vec<Value> = all
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "n": i + 1, "id": r.id, "part": part_of_saved(r), "state": r.state(),
                "title": r.headline(), "target": crash_cmd::target_of(&env.root, r),
                "issue": r.issue.as_ref().map(|x| x.url.clone()),
                "sections": sections_of(r),
            })
        })
        .collect();
    let data = json!({ "reports": rows });
    if all.is_empty() {
        return Ok(Report::done("no bug reports are saved on this machine")
            .next("ciginstaller bug")
            .data(data));
    }
    let mut table = cigi_ui::Table::new(&[
        ("n", cigi_ui::Align::Right),
        ("report", cigi_ui::Align::Left),
        ("part", cigi_ui::Align::Left),
        ("state", cigi_ui::Align::Left),
        ("what", cigi_ui::Align::Left),
    ]);
    for (i, r) in all.iter().enumerate() {
        table = table.row(vec![
            cigi_ui::Cell::new((i + 1).to_string()),
            cigi_ui::Cell::new(r.id.clone()),
            cigi_ui::Cell::new(flow::label_of(&part_of_saved(r))),
            cigi_ui::Cell::new(r.state()),
            cigi_ui::Cell::new(crash::truncate(r.headline(), 60)),
        ]);
    }
    let mut rep = Report::done(format!("{} bug report(s) saved here", all.len()));
    for l in super::tabular::details(env.ui, &table) {
        rep = rep.detail(l);
    }
    Ok(rep.next("ciginstaller bug show <n>").data(data))
}

fn show(env: &crash_cmd::Env, n: &str) -> Result<Report, InstallerError> {
    let all = mine(env)?;
    let pick = n
        .parse::<usize>()
        .ok()
        .filter(|i| (1..=all.len()).contains(i))
        .map(|i| &all[i - 1])
        .or_else(|| {
            let m: Vec<&Saved> = all.iter().filter(|r| r.id.starts_with(n)).collect();
            (m.len() == 1 && !n.is_empty()).then(|| m[0])
        });
    let Some(r) = pick else {
        return Err(usage(format!("no bug report here is `{n}`")).with_fix("ciginstaller bug list"));
    };
    let v = flow::view(r, &env.root);
    let mut rep = Report::done(format!("bug report {} ({})", r.id, r.state()));
    rep = rep.detail(if r.filed() {
        "this is what was sent:".to_string()
    } else {
        "this is exactly what `ciginstaller crash send` would send:".to_string()
    });
    for l in flow::preview_lines(&v) {
        rep = rep.detail(l);
    }
    Ok(rep
        .next(format!("ciginstaller crash send {}", r.id))
        .data(json!({ "id": r.id, "state": r.state(), "target": crash_cmd::target_of(&env.root, r), "report": v })))
}

// ----- the conversation --------------------------------------------------------------------

/// One text answer: the flag, else a typed line on a terminal, else empty.
fn text_answer(ui: &Ui, flag: Option<&String>, q: &Msg, interactive: bool) -> String {
    if let Some(t) = flag {
        return t.trim().to_string();
    }
    if interactive {
        return ui.ask_text(&ui.msg(q, &[])).unwrap_or_default();
    }
    String::new()
}

fn kind_word(kind: &str) -> &'static str {
    match kind {
        "path" => "a path",
        "address" => "an address",
        "token" => "a token",
        "email" => "an e-mail address",
        _ => "a name of yours",
    }
}

/// What was decided about the marked spans of all the answers: the spans kept, one entry per
/// mark for the output, and one line per mark for a person.
#[derive(Default)]
struct Decisions {
    kept: Vec<String>,
    marks: Vec<Value>,
    lines: Vec<String>,
}

/// Decide every marked span of one answer. Interactive: each is asked, the default is to replace.
/// Otherwise they are replaced, unless `--keep-marked`. A token is always replaced. Every
/// decision is logged for the output; the text is never changed without a line saying so.
fn decide(
    ui: &Ui,
    field: &str,
    text: &str,
    root: &std::path::Path,
    how: (bool, bool),
    out: &mut Decisions,
) -> Result<String, InstallerError> {
    let (interactive, keep_all) = how;
    let Decisions {
        kept,
        marks: log,
        lines,
    } = out;
    let marks = anon::marks(text, Some(root));
    if marks.is_empty() {
        return Ok(text.to_string());
    }
    let mut keep_these: Vec<String> = Vec::new();
    for m in &marks {
        let keep = if !m.keepable() {
            let said = ui.msg(&SAID_TOKEN, &[kind_word(m.kind), &m.text, &m.replace]);
            if interactive {
                ui.say(&Report::new(Icon::Note, said));
            }
            false
        } else if interactive {
            let q = question(
                ui.msg(&Q_MARK, &[kind_word(m.kind), &m.text, &m.replace]),
                vec![Choice::new('k', "keep it"), Choice::new('r', "replace it")],
                1,
            );
            asked(ui, &q)? == 0
        } else {
            keep_all
        };
        if keep {
            keep_these.push(m.text.clone());
        }
        log.push(json!({
            "field": field, "kind": m.kind, "kept": keep,
            "shown": if keep { m.text.clone() } else { m.replace.clone() },
        }));
        lines.push(format!(
            "{field}: {} `{}` {}",
            kind_word(m.kind),
            m.text,
            if keep {
                "kept as typed".to_string()
            } else {
                format!("replaced with `{}`", m.replace)
            }
        ));
    }
    let shown = anon::apply_marks(text, &marks, &|m| keep_these.contains(&m.text));
    kept.extend(keep_these);
    Ok(shown)
}

struct Facts {
    part: String,
    run: Option<String>,
}

pub fn run(
    ui: &Ui,
    globals: Globals,
    cwd: Option<PathBuf>,
    c: &BugCmd,
) -> Result<Report, InstallerError> {
    let env = crash_cmd::open_env(ui, globals.home.as_ref())?;
    match &c.sub {
        Some(BugSub::List) => return list(&env),
        Some(BugSub::Show { n }) => return show(&env, n),
        None => {}
    }
    write_up(ui, &env, globals, cwd, c)
}

fn write_up(
    ui: &Ui,
    env: &crash_cmd::Env,
    globals: Globals,
    cwd: Option<PathBuf>,
    c: &BugCmd,
) -> Result<Report, InstallerError> {
    // flags that can be wrong are checked before a person is asked anything
    let removed = flow::removal(&c.without)?;
    let every_flag = match c.every_time.as_deref().map(str::trim) {
        None => None,
        Some(w @ ("yes" | "no" | "unsure")) => Some(w.to_string()),
        Some(w) => return Err(usage(format!("`{w}` is not yes, no or unsure"))),
    };
    let flagged_part = match product_of(c).or(c.part.as_deref()) {
        Some(p) => Some(flow::part_of(p)?),
        None => None,
    };
    if c.send && crash::explicit_never(&env.cfg) {
        return Err(crash_cmd::never_error(env, None));
    }
    let interactive = can_ask(ui);
    let root = env.root.as_path();

    let cx = Cx::new(ui.duplicate(), globals, cwd);
    let (mut cx, broken) = match cx {
        Ok(cx) => (Some(cx), None),
        Err(e) if c.run.is_some() || c.last => return Err(e),
        Err(e) => (None, Some(e)),
    };

    if interactive {
        ui.say(&Report::new(Icon::Note, ui.msg(&OPENER, &[])));
    }

    // 1. which part
    let mut part: &'static str = flagged_part.unwrap_or("unsure");
    if flagged_part.is_none() && interactive {
        let mut choices: Vec<Choice> = flow::PARTS
            .iter()
            .map(|(_, l, k)| Choice::new(*k, l))
            .collect();
        choices.push(Choice::new('s', "skip"));
        let skip = choices.len() - 1;
        let i = asked(ui, &question(ui.msg(&Q_PART, &[]), choices, skip))?;
        part = flow::PARTS.get(i).map_or("unsure", |(p, _, _)| *p);
    }

    // 2-4. in their words
    let did = text_answer(ui, c.did.as_ref(), &Q_DID, interactive);
    let expected = text_answer(ui, c.expected.as_ref(), &Q_EXPECTED, interactive);
    let happened = text_answer(ui, c.happened.as_ref(), &Q_HAPPENED, interactive);

    // 5. which run
    let mut facts = Facts {
        part: part.to_string(),
        run: c.run.clone(),
    };
    if c.last {
        let cx = cx.as_ref().ok_or_else(|| usage("no state to look in"))?;
        match flow::last_failed(cx) {
            Some((id, _, _)) => facts.run = Some(id),
            None => {
                return Err(usage(
                    "nothing has failed on this machine that a report could be about",
                )
                .with_fix("ciginstaller history"))
            }
        }
    } else if facts.run.is_none() && product_of(c).is_none() && interactive {
        if let Some(cx) = cx.as_ref() {
            facts.run = ask_run(ui, cx)?;
        }
    }

    // the evidence
    let evidence = flow::gather(
        cx.as_mut(),
        broken.as_ref(),
        facts.run.as_deref(),
        &facts.part,
        root,
    )?;

    // not sure which part: the run and its error code say
    if facts.part == "unsure" {
        let from_op = evidence
            .run
            .as_ref()
            .is_some_and(|r| r.source == "installer");
        let from_cig = evidence.run.as_ref().is_some_and(|r| r.source == "cig");
        if let Some(p) = flow::suggest_part(evidence.code.as_deref(), from_op, from_cig) {
            let why = evidence.code.clone().map_or_else(
                || "the run".to_string(),
                |code| format!("the error code {code}"),
            );
            let take = if interactive {
                let q = question(
                    ui.msg(&Q_SUGGEST, &[flow::label_of(p), &why]),
                    vec![Choice::new('y', "yes"), Choice::new('n', "no")],
                    0,
                );
                asked(ui, &q)? == 0
            } else {
                true
            };
            if take {
                facts.part = p.to_string();
            }
        }
    }
    let mut evidence = evidence;
    if facts.part != "unsure" && facts.part != part {
        if let Some(cx) = cx.as_mut() {
            let mut d = flow::doctor_for(cx, &facts.part);
            anon::anonymise_value(&mut d, Some(root));
            evidence.doctor = d;
        }
    }

    // every time? only when nothing verified it
    let mut every_time = String::new();
    if !evidence.repro.verified() {
        if let Some(w) = every_flag {
            every_time = w;
        } else if interactive {
            let q = question(
                ui.msg(&Q_EVERY, &[]),
                vec![
                    Choice::new('y', "yes"),
                    Choice::new('n', "no"),
                    Choice::new('u', "not sure"),
                    Choice::new('s', "skip"),
                ],
                3,
            );
            every_time = ["yes", "no", "unsure", ""][asked(ui, &q)?].to_string();
        }
    }

    // their words: marked, decided, never silently rewritten
    let mut decided = Decisions::default();
    let mut said = flow::Answers {
        every_time,
        ..Default::default()
    };
    for (field, text, slot) in [
        ("did", &did, &mut said.did),
        ("expected", &expected, &mut said.expected),
        ("happened", &happened, &mut said.happened),
    ] {
        *slot = decide(
            ui,
            field,
            text,
            root,
            (interactive, c.keep_marked),
            &mut decided,
        )?;
    }
    said.kept = std::mem::take(&mut decided.kept);
    let (marks, mark_lines) = (decided.marks, decided.lines);

    let mut draft = flow::Draft {
        part: facts.part.clone(),
        answers: said,
        evidence,
        removed,
    };

    if !interactive {
        return finish_flags(ui, env, c, &draft, marks, mark_lines);
    }

    // the preview, then the one question; a section can come out and the preview is redrawn
    loop {
        let saved = flow::compose(&draft, root);
        let v = flow::view(&saved, root);
        let mut shown = Report::new(Icon::Action, "the report, exactly as it would travel:");
        for l in flow::preview_lines(&v) {
            shown = shown.detail(l);
        }
        ui.say(&shown);
        let never = crash::explicit_never(&env.cfg);
        let mut choices = Vec::new();
        if !never {
            choices.push(Choice::new('s', "send it"));
        }
        choices.push(Choice::new('k', "keep it here"));
        choices.push(Choice::new('t', "take a section out"));
        choices.push(Choice::new('c', "cancel"));
        let keep_at = usize::from(!never);
        let mut q = question(ui.msg(&Q_FINAL, &[]), choices.clone(), keep_at);
        if never {
            q = q.detail(
                "reports is \"never\", so nothing can be sent from here; it can still be kept",
            );
        }
        let word = choices[asked(ui, &q)?].label.clone();
        match word.as_str() {
            "send it" => return save(env, c, saved, true, marks, mark_lines),
            "keep it here" => return save(env, c, saved, false, marks, mark_lines),
            "take a section out" => {
                let typed = ui.ask_text(&ui.msg(&Q_SECTION, &[])).unwrap_or_default();
                if typed.is_empty() {
                    continue;
                }
                match flow::removal(std::slice::from_ref(&typed)) {
                    Ok(s) => draft.removed.extend(s),
                    Err(e) => ui.error(&e),
                }
            }
            _ => {
                return Ok(
                    Report::new(Icon::Note, "cancelled: nothing was saved or sent")
                        .data(json!({ "saved": false, "sent": false })),
                )
            }
        }
    }
}

fn ask_run(ui: &Ui, cx: &Cx) -> Result<Option<String>, InstallerError> {
    let last = flow::last_failed(cx);
    let mut choices = Vec::new();
    if let Some((id, when, what)) = &last {
        choices.push(Choice::new('r', &format!("{id} ({when}, {what})")));
    }
    choices.push(Choice::new('a', "another"));
    choices.push(Choice::new('n', "none"));
    let none = choices.len() - 1;
    let i = asked(ui, &question(ui.msg(&Q_RUN, &[]), choices, none))?;
    if last.is_some() && i == 0 {
        return Ok(last.map(|l| l.0));
    }
    if i == none {
        return Ok(None);
    }
    let typed = ui.ask_text(&ui.msg(&Q_ANOTHER, &[])).unwrap_or_default();
    Ok((!typed.is_empty()).then_some(typed))
}

/// No terminal: the preview, and a save or a send only when a flag says so.
fn finish_flags(
    ui: &Ui,
    env: &crash_cmd::Env,
    c: &BugCmd,
    draft: &flow::Draft,
    marks: Vec<Value>,
    mark_lines: Vec<String>,
) -> Result<Report, InstallerError> {
    let saved = flow::compose(draft, &env.root);
    if c.save || c.send {
        return save(env, c, saved, c.send, marks, mark_lines);
    }
    let v = flow::view(&saved, &env.root);
    let mut rep = Report::done("report drafted: nothing was saved and nothing was sent");
    for l in &mark_lines {
        rep = rep.detail(l.clone());
    }
    rep = rep.detail("this is exactly what would travel:".to_string());
    if ui.mode != Mode::Json {
        for l in flow::preview_lines(&v) {
            rep = rep.detail(l);
        }
    }
    Ok(rep
        .next("add --save to keep it here, or --send to send it")
        .data(json!({
            "saved": false, "sent": false, "id": saved.id, "part": draft.part,
            "marks": marks, "report": v,
        })))
}

/// The yes: the report joins the ashtray as kind `report`; sending goes through `crash send`.
fn save(
    env: &crash_cmd::Env,
    _c: &BugCmd,
    mut saved: Saved,
    send: bool,
    marks: Vec<Value>,
    mark_lines: Vec<String>,
) -> Result<Report, InstallerError> {
    saved.queue.requested = send;
    crash::write_new(&env.root, &mut saved)
        .map_err(|e| crate::flows::report::failed(format!("could not save the report: {e}")))?;
    let v = flow::view(&saved, &env.root);
    let part = saved
        .details
        .as_ref()
        .and_then(|d| d["part"].as_str())
        .unwrap_or("unsure")
        .to_string();
    let mine = json!({
        "saved": true, "id": saved.id, "part": part, "marks": marks, "report": v,
    });
    if send {
        let mut r = crash_cmd::send(env, Some(&saved.id), false)?;
        for l in mark_lines.into_iter().rev() {
            r.detail.insert(0, l);
        }
        let mut data = match r.data.take() {
            Value::Object(o) => o,
            _ => serde_json::Map::new(),
        };
        data.insert("bug".into(), mine);
        r.data = Value::Object(data);
        return Ok(r);
    }
    let mut rep = Report::done(format!(
        "saved as {}: it stays here until you send it",
        saved.id
    ));
    for l in mark_lines {
        rep = rep.detail(l);
    }
    Ok(rep
        .next(format!("ciginstaller crash send {}", saved.id))
        .data(json!({ "saved": true, "sent": false, "bug": mine })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cigi_ui::{Env, GlobalFlags};

    fn ui(tty: bool) -> (Ui, cigi_ui::Capture) {
        let env = Env {
            tty_out: tty,
            tty_err: tty,
            tty_in: tty,
            ..Env::default()
        };
        Ui::from_env(&GlobalFlags::default(), &env).captured()
    }

    #[test]
    fn nothing_is_asked_without_a_terminal() {
        let (u, cap) = ui(false);
        cap.push_input("an answer nobody should read");
        assert!(!can_ask(&u));
        assert_eq!(u.ask_text("2. what were you doing?"), None);
        assert_eq!(text_answer(&u, None, &Q_DID, false), "");
        assert_eq!(
            text_answer(&u, Some(&"from a flag".to_string()), &Q_DID, false),
            "from a flag"
        );
    }

    #[test]
    fn every_question_can_be_skipped() {
        let (u, cap) = ui(true);
        assert!(can_ask(&u));
        // an empty line skips a text question, in the voice and in plain
        for q in [&Q_DID, &Q_EXPECTED, &Q_HAPPENED] {
            cap.push_input("");
            assert_eq!(text_answer(&u, None, q, true), "");
        }
        // the end of the input is a skip too
        assert_eq!(text_answer(&u, None, &Q_DID, true), "");
        // the part question has a skip, and it is the default
        let mut choices: Vec<Choice> = flow::PARTS
            .iter()
            .map(|(_, l, k)| Choice::new(*k, l))
            .collect();
        choices.push(Choice::new('s', "skip"));
        let skip = choices.len() - 1;
        cap.push_input("s");
        assert_eq!(
            asked(
                &u,
                &question("1. which part?".into(), choices.clone(), skip)
            )
            .unwrap(),
            skip
        );
        cap.push_input("");
        assert_eq!(
            asked(&u, &question("1. which part?".into(), choices, skip)).unwrap(),
            skip
        );
        // both forms of the opener are real sentences
        assert!(OPENER.voice != OPENER.plain && !OPENER.voice.contains('!'));
    }

    #[test]
    fn a_marked_span_is_kept_only_when_the_person_says_so() {
        let root = std::env::temp_dir();
        let said = "see /srv/plans/q3/notes.txt now";
        let (u, cap) = ui(true);
        let mut d = Decisions::default();
        cap.push_input("k");
        let out = decide(&u, "did", said, &root, (true, false), &mut d).unwrap();
        assert_eq!(out, said);
        assert_eq!(d.kept, vec!["/srv/plans/q3/notes.txt".to_string()]);
        // the default (enter) replaces
        let mut d = Decisions::default();
        cap.push_input("");
        let out = decide(&u, "did", said, &root, (true, false), &mut d).unwrap();
        assert!(!out.contains("/srv/plans") && d.kept.is_empty(), "{out}");
        assert_eq!(d.lines.len(), 1, "the replacement is listed");
        // a token is not offered and is replaced even when everything is kept
        let mut d = Decisions::default();
        let tok = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let out = decide(
            &u,
            "happened",
            &format!("used {tok}"),
            &root,
            (true, true),
            &mut d,
        )
        .unwrap();
        assert!(!out.contains(tok) && d.kept.is_empty());
    }
}
