// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

mod common;
use cig_compat::*;
use common::*;

fn combo(list: &[(&str, &str)], origin: Origin) -> Combination {
    Combination {
        items: items(list),
        origin,
    }
}

fn only(fs: &[Finding], code: &str) -> Vec<Finding> {
    fs.iter().filter(|f| f.code == code).cloned().collect()
}

/// The voice grammar of #32, enough to compare with #4's example (the real
/// renderer lives in cigi-ui).
fn voice(f: &Finding) -> String {
    let mut s = format!("{:<60} {}\n", f.headline, f.code);
    for fix in &f.fixes {
        s.push_str(&format!("  → {fix}\n"));
    }
    s
}

#[test]
fn a_whole_carton_is_clean() {
    let m = fleet();
    let c = combo(
        &[("cig", "1.8.0"), ("cigo", "1.4.0")],
        Origin::Carton(v("1.1.2")),
    );
    assert_eq!(check(&m, &c, &linux_host("6.17.0-1033-oem")), vec![]);
    let m = spec_manifest();
    let c = combo(
        &[("cig", "1.20.0"), ("cigo", "1.17.0")],
        Origin::Carton(v("0.1.0")),
    );
    assert_eq!(check(&m, &c, &linux_host("6.1.0")), vec![]);
}

#[test]
fn e1801_a_product_needs_another_golden() {
    let m = fleet();
    let c = combo(&[("cig", "1.7.2"), ("cigo", "1.4.0")], Origin::Mixed);
    let fs = only(&check(&m, &c, &linux_host("6.1.0")), "E1801");
    assert_eq!(fs.len(), 1);
    let f = &fs[0];
    assert_eq!(f.severity, Severity::Refuse);
    assert_eq!(f.subject, "cigo");
    assert_eq!(f.fixes.len(), 2);
    assert_eq!(fix_command(&f.fixes[0]), "ciginstaller update");
    assert_eq!(fix_command(&f.fixes[1]), "ciginstaller pin carton 1.0");
    let golden = include_str!("golden/e1801.txt");
    assert_eq!(voice(f), golden);
}

#[test]
fn e1801_the_range_in_words() {
    let m = spec_manifest();
    let c = combo(&[("cig", "1.10.0"), ("cigo", "1.17.0")], Origin::Mixed);
    let fs = check(&m, &c, &linux_host("6.1.0"));
    let f = &only(&fs, "E1801")[0];
    assert_eq!(
        f.headline,
        "cigo 1.17.0 needs cig 1.16.1 or newer, below 2; you have 1.10.0"
    );
    // the same, but cig 2.0.0 is above the ceiling
    let c = combo(&[("cig", "2.0.0"), ("cigo", "1.17.0")], Origin::Mixed);
    assert_eq!(only(&check(&m, &c, &linux_host("6.1.0")), "E1801").len(), 1);
}

#[test]
fn e1801_soft_requirements_warn() {
    let m = fleet();
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.3.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = only(&check(&m, &c, &linux_host("6.1.0")), "E1801");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Warn);
    assert!(fs[0].headline.contains("works best with"));
}

#[test]
fn e1801_a_missing_dependency_is_not_guessed_at() {
    let m = fleet();
    let c = combo(&[("cigo", "1.4.0")], Origin::Mixed);
    assert!(only(&check(&m, &c, &linux_host("6.1.0")), "E1801").is_empty());
}

#[test]
fn e1802_a_stick_does_not_fit_its_carton() {
    let m = fleet();
    let c = combo(
        &[("cig", "1.8.0"), ("cigo", "1.3.0")],
        Origin::Stick { carton: v("1.1.2") },
    );
    let fs = only(&check(&m, &c, &linux_host("6.1.0")), "E1802");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Warn);
    assert_eq!(fs[0].subject, "cigo");
    assert!(fs[0].headline.contains("cigo 1.3.0") && fs[0].headline.contains("(1.4.0)"));
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller carton 1.1.2");
}

#[test]
fn e1803_yanked_versions_always_warn_with_their_reason() {
    let m = fleet();
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.3.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = only(&check(&m, &c, &linux_host("6.1.0")), "E1803");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Warn);
    assert_eq!(fs[0].subject, "carton 0.9.0");
    assert!(fs[0].headline.ends_with("rollback skipped a file"));
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller carton 1.0.0");
    // a yanked product, even though the manifest has no build of it
    let c = combo(&[("cigo", "1.4.1")], Origin::Mixed);
    let fs = only(&check(&m, &c, &linux_host("6.1.0")), "E1803");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].subject, "cigo 1.4.1");
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller update");
}

#[test]
fn e1804_advisories_name_the_fixed_version() {
    let m = fleet();
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.3.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = only(&check(&m, &c, &linux_host("6.1.0")), "E1804");
    let subjects: Vec<&str> = fs.iter().map(|f| f.subject.as_str()).collect();
    assert_eq!(
        subjects,
        ["CIGI-2026-0001 carton 0.9.0", "CIGI-2026-0002 cig 1.6.0"]
    );
    assert!(fs.iter().all(|f| f.severity == Severity::Warn));
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller carton 1.0.0");
    assert_eq!(fix_command(&fs[1].fixes[0]), "ciginstaller update");
    assert!(fs[0]
        .detail
        .contains(&"https://example.invalid/advisory".to_string()));
    // not affected: nothing
    let c = combo(
        &[("cig", "1.8.0"), ("cigo", "1.4.0")],
        Origin::Carton(v("1.1.2")),
    );
    assert!(only(&check(&m, &c, &linux_host("6.1.0")), "E1804").is_empty());
}

#[test]
fn e1805_a_compiled_program_against_the_installed_cig() {
    let m = spec_manifest();
    // spec example: cig 1.20.0 embeds no journal format, so only a note about that
    let fs = check_program(&m, &v("1.18.0"), 3, &v("1.20.0"));
    assert_eq!(only(&fs, "E1805"), vec![]);
    let older = check_program(&m, &v("1.18.0"), 3, &v("1.16.0"));
    let f = &only(&older, "E1805")[0];
    assert_eq!(f.severity, Severity::Refuse);
    assert!(f.headline.contains("older"));
    let major = check_program(&m, &v("1.18.0"), 3, &v("2.0.0"));
    assert!(only(&major, "E1805")[0]
        .headline
        .contains("different major"));
}

#[test]
fn e1806_the_journal_format_against_this_cig() {
    let m = fleet();
    let newer = check_journal(&m, 4, &v("1.8.0"));
    assert_eq!(
        (newer[0].code, newer[0].severity),
        ("E1806", Severity::Refuse)
    );
    assert!(newer[0].headline.contains("newer"));
    assert_eq!(fix_command(&newer[0].fixes[0]), "ciginstaller update");
    assert!(check_journal(&m, 3, &v("1.8.0")).is_empty());
    let older = check_journal(&m, 2, &v("1.8.0"));
    assert_eq!(older[0].severity, Severity::Note);
    // a cig the manifest knows nothing about: a note, never a guess
    let unknown = check_journal(&m, 3, &v("9.9.9"));
    assert_eq!(unknown[0].severity, Severity::Note);
    // the program row reaches the same rule
    let fs = check_program(&m, &v("1.7.0"), 4, &v("1.8.0"));
    assert_eq!(only(&fs, "E1806").len(), 1);
}

#[test]
fn e1807_os_minimums_are_evaluated_only_for_kernel_x_y() {
    let m = fleet();
    let c = combo(&[("cig", "1.8.0")], Origin::Carton(v("1.1.2")));
    let fs = only(&check(&m, &c, &linux_host("4.19.0-12-generic")), "E1807");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Refuse);
    assert!(fs[0].headline.contains("kernel 5.4") && fs[0].headline.contains("4.19.0-12-generic"));
    assert!(only(&check(&m, &c, &linux_host("5.4.0")), "E1807").is_empty());
    assert!(only(&check(&m, &c, &linux_host("5.10")), "E1807").is_empty());
    assert!(only(&check(&m, &c, &linux_host("5.3.99")), "E1807").len() == 1);
    // windows "10" is free text: not evaluated
    let mut host = linux_host("1.0");
    host.os = "windows".into();
    host.os_version = Some("7".into());
    assert!(only(&check(&m, &c, &host), "E1807").is_empty());
    // no kernel known: not evaluated
    let mut host = linux_host("1.0");
    host.kernel = None;
    assert!(only(&check(&m, &c, &host), "E1807").is_empty());
}

#[test]
fn e1808_a_packages_requirements() {
    let have = items(&[("cig", "1.8.0"), ("cigcompile", "0.2.0")]);
    let ok = check_package("pkg", &[("cigcompile".into(), ">=0.2".into())], &have);
    assert!(ok.is_empty());
    let low = check_package("pkg", &[("cigcompile".into(), ">=0.3".into())], &have);
    assert_eq!((low[0].code, low[0].severity), ("E1808", Severity::Refuse));
    assert_eq!(
        low[0].headline,
        "pkg needs cigcompile 0.3 or newer; you have 0.2.0"
    );
    let none = check_package("pkg", &[("cigarscript".into(), ">=0.1".into())], &have);
    assert!(none[0].headline.contains("none is installed"));
    let bad = check_package("pkg", &[("cig".into(), "wat".into())], &have);
    assert!(bad[0].headline.contains("not a version range"));
}

#[test]
fn e1811_e1814_e1815_e1800_the_plumbing_rows() {
    let m = fleet();
    let host = linux_host("6.1.0");
    let c = combo(&[("cig", "1.8.0")], Origin::Carton(v("7.0.0")));
    assert_eq!(
        only(&check(&m, &c, &host), "E1811")[0].severity,
        Severity::Refuse
    );
    let c = combo(&[("cigar", "0.1.0")], Origin::Mixed);
    assert_eq!(
        only(&check(&m, &c, &host), "E1814")[0].severity,
        Severity::Warn
    );
    let c = combo(&[("cig", "1.9.9")], Origin::Mixed);
    assert_eq!(
        only(&check(&m, &c, &host), "E1800")[0].severity,
        Severity::Note
    );
    // no build for this platform: only when installing
    let mut win = linux_host("1.0");
    win.os = "windows".into();
    let c = combo(&[("cig", "1.8.0")], Origin::Carton(v("1.1.2")));
    assert_eq!(
        only(&check(&m, &c, &win), "E1815")[0].severity,
        Severity::Refuse
    );
    let c = combo(&[("cig", "1.8.0")], Origin::Mixed);
    assert!(only(&check(&m, &c, &win), "E1815").is_empty());
}

#[test]
fn findings_come_worst_first_and_worst_is_reported() {
    let m = fleet();
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.4.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = check(&m, &c, &linux_host("6.1.0"));
    assert!(fs.windows(2).all(|w| w[0].severity <= w[1].severity));
    assert_eq!(worst(&fs), Some(Severity::Refuse));
    assert_eq!(worst(&[]), None);
}

#[test]
fn project_pins_against_installed_and_each_other() {
    let m = fleet();
    let host = linux_host("6.1.0");
    let pins = vec![("cigo".to_string(), Pin::parse("1.3").unwrap())];
    // installed has cigo 1.4.0 only: the pin matches nothing installed
    let fs = check_pins(
        &m,
        &pins,
        &items(&[("cig", "1.8.0"), ("cigo", "1.4.0")]),
        &host,
    );
    let f = &only(&fs, "E1800")[0];
    assert_eq!(f.severity, Severity::Warn);
    assert!(f.headline.contains("pins cigo 1.3") && f.headline.contains("1.4.0"));
    assert_eq!(fix_command(&f.fixes[0]), "ciginstaller carton 1.0.0");
    // satisfied: clean of E1800
    let fs = check_pins(
        &m,
        &pins,
        &items(&[("cig", "1.7.2"), ("cigo", "1.3.0")]),
        &host,
    );
    assert!(only(&fs, "E1800").is_empty());
    // two pins no version satisfies
    let pins = vec![
        ("cigo".to_string(), Pin::parse("1.3").unwrap()),
        ("cigo".to_string(), Pin::parse(">=1.4.0").unwrap()),
    ];
    let fs = check_pins(&m, &pins, &items(&[("cigo", "1.4.0")]), &host);
    assert_eq!(only(&fs, "E1800")[0].severity, Severity::Refuse);
    // the combination the pins resolve to is checked too: cigo 1.4.0 needs cig 1.8
    let pins = vec![("cigo".to_string(), Pin::parse("1.4").unwrap())];
    let fs = check_pins(
        &m,
        &pins,
        &items(&[("cig", "1.7.2"), ("cigo", "1.4.0")]),
        &host,
    );
    assert_eq!(only(&fs, "E1801").len(), 1);
}

// ---- legacy_resolve (#6)

fn pins(list: &[(&str, &str)]) -> Vec<(String, Pin)> {
    list.iter()
        .map(|(t, p)| (t.to_string(), Pin::parse(p).unwrap()))
        .collect()
}

#[test]
fn legacy_a_header_pin_gets_a_satisfying_carton() {
    let m = fleet();
    // "# cig >=1.6.2": cartons 1.0.0 (1.7.2) and 1.1.2 (1.8.0) both satisfy; the newest installed wins
    let both = [v("1.0.0"), v("1.1.2")];
    assert_eq!(
        legacy_resolve(&m, &both, &pins(&[("cig", ">=1.6.2")])),
        LegacyOutcome::Carton(v("1.1.2"))
    );
    let old = [v("0.9.0"), v("1.0.0")];
    assert_eq!(
        legacy_resolve(&m, &old, &pins(&[("cig", ">=1.6.2")])),
        LegacyOutcome::Carton(v("1.0.0"))
    );
    assert_eq!(
        legacy_resolve(&m, &both, &pins(&[("cig", "1.7"), ("cigo", "1.3")])),
        LegacyOutcome::Carton(v("1.0.0"))
    );
    assert_eq!(
        legacy_resolve(&m, &both, &[]),
        LegacyOutcome::Carton(v("1.1.2"))
    );
}

#[test]
fn legacy_no_carton_satisfies_so_a_stick() {
    let m = fleet();
    let only_new = [v("1.1.2")];
    assert_eq!(
        legacy_resolve(&m, &only_new, &pins(&[("cigo", "1.3")])),
        LegacyOutcome::Stick {
            tool: "cigo".into(),
            carton: v("1.0.0")
        }
    );
    // a yanked carton is the stick's source only when nothing else carries it
    assert_eq!(
        legacy_resolve(&m, &only_new, &pins(&[("cig", "1.6")])),
        LegacyOutcome::Stick {
            tool: "cig".into(),
            carton: v("0.9.0")
        }
    );
}

#[test]
fn legacy_nothing_works() {
    let m = fleet();
    let have = [v("1.1.2")];
    let LegacyOutcome::None { why } = legacy_resolve(&m, &have, &pins(&[("cig", "3")])) else {
        panic!("expected none");
    };
    assert!(why[0].contains("cig 3") && why.last().unwrap().contains("no carton"));
    // two tools off at once is more than one stick
    let LegacyOutcome::None { why } =
        legacy_resolve(&m, &have, &pins(&[("cig", "1.7"), ("cigo", "1.3")]))
    else {
        panic!("expected none");
    };
    assert!(why.last().unwrap().contains("more than one"));
    let LegacyOutcome::None { why } = legacy_resolve(&m, &[], &pins(&[("cig", "1")])) else {
        panic!()
    };
    assert!(why[0].contains("no installed"));
    // a tool no carton has
    assert!(matches!(
        legacy_resolve(&m, &have, &pins(&[("cigar", "1")])),
        LegacyOutcome::None { .. }
    ));
}
