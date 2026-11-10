// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `every_message_has_both_forms` (#31): the catalogue holds the voice and plain together,
//! with the same placeholders, and keeps the voice's rules.

use cigi_ui::catalogue::{self, Msg};
use cigi_ui::registry;
use std::collections::BTreeSet;

fn placeholders(s: &str) -> BTreeSet<String> {
    s.split('{')
        .skip(1)
        .filter_map(|r| r.split('}').next())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_message_has_both_forms() {
    let mut ids = BTreeSet::new();
    for m in catalogue::ALL {
        assert!(ids.insert(m.id), "{} is listed twice", m.id);
        assert!(
            !m.voice.trim().is_empty() && !m.plain.trim().is_empty(),
            "{}",
            m.id
        );
        assert_eq!(
            placeholders(m.voice),
            placeholders(m.plain),
            "{}: both forms take the same arguments",
            m.id
        );
    }
    assert!(catalogue::ALL.len() >= 12);
}

#[test]
fn the_voice_keeps_its_rules() {
    for m in catalogue::ALL {
        for (form, text) in [("voice", m.voice), ("plain", m.plain)] {
            assert!(!text.contains('!'), "{} {form}: no exclamation marks", m.id);
            let lower = text.to_ascii_lowercase();
            for banned in ["simply", "just ", "easy", "easily"] {
                assert!(!lower.contains(banned), "{} {form}: no \"{banned}\"", m.id);
            }
        }
    }
}

#[test]
fn plain_names_the_same_facts_with_no_slang() {
    for m in catalogue::ALL {
        assert!(
            !m.plain.contains("ciggies"),
            "{}: plain has no catchphrase",
            m.id
        );
    }
    let m = catalogue::NO_TERMINAL;
    let args = ["no", "update?", "pass --yes to say yes"];
    let (voice, plain) = (m.render(false, &args), m.render(true, &args));
    for fact in args {
        assert!(voice.contains(fact) && plain.contains(fact), "{fact}");
    }
}

#[test]
fn the_four_kinds_of_no_match_the_registry() {
    for id in [
        "cant-see-any",
        "these-are-mine",
        "dont-have-any",
        "never-heard-of-you",
    ] {
        let no = registry::lookup_no(id).expect(id);
        let m: &Msg = catalogue::kind_of_no(id).expect(id);
        assert_eq!(
            m.voice, no.hear,
            "{id}: the voice is the family's catchphrase"
        );
        assert_ne!(m.plain, m.voice);
    }
    assert!(catalogue::kind_of_no("none").is_none());
    assert!(catalogue::kind_of_no("depends").is_none());
}
