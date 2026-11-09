// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
mod common;
use cig_compat::{Manifest, Pin, Version};
use proptest::prelude::*;

fn pre_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        "(alpha|beta|rc|x-y)(\\.([1-9][0-9]{0,3}|0|[a-z]{1,3})){0,2}".prop_map(Some),
    ]
}

fn version_strategy() -> impl Strategy<Value = Version> {
    (0u64..50, 0u64..50, 0u64..50, pre_strategy()).prop_map(|(major, minor, patch, pre)| Version {
        major,
        minor,
        patch,
        pre,
    })
}

proptest! {
    #[test]
    fn version_display_parse_round_trip(v in version_strategy()) {
        prop_assert_eq!(Version::parse(&v.to_string()).unwrap(), v.clone());
        prop_assert_eq!(Version::parse(&format!("v{v}")).unwrap(), v);
    }

    #[test]
    fn version_order_is_total_and_agrees_with_semver(a in version_strategy(), b in version_strategy()) {
        let (sa, sb) = (semver::Version::parse(&a.to_string()).unwrap(), semver::Version::parse(&b.to_string()).unwrap());
        prop_assert_eq!(a.cmp(&b), sa.cmp(&sb));
        prop_assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
        prop_assert_eq!(a == b, a.cmp(&b) == std::cmp::Ordering::Equal);
    }

    #[test]
    fn version_parse_never_panics(s in ".{0,40}") {
        let _ = Version::parse(&s);
        let _ = Pin::parse(&s);
        let _ = Pin::parse_carton_line(&s);
    }

    #[test]
    fn pin_display_parse_round_trip(a in 0u64..30, b in 0u64..30, c in 0u64..30, kind in 0u8..4) {
        let text = match kind {
            0 => format!("{a}"),
            1 => format!("{a}.{b}"),
            2 => format!("{a}.{b}.{c}"),
            _ => format!(">={a}.{b}.{c}"),
        };
        let p = Pin::parse(&text).unwrap();
        prop_assert_eq!(p.to_string(), text);
        prop_assert!(p.satisfied_by(&Version::new(a, b, c)));
    }

    #[test]
    fn ranges_never_cross_a_major_version(m in 0u64..9, n in 0u64..9, v in version_strategy()) {
        let p = Pin::parse(&format!(">={m}.{n}.0")).unwrap();
        if p.satisfied_by(&v) {
            prop_assert_eq!(v.major, m);
            prop_assert!(v.pre.is_none());
        }
    }

    #[test]
    fn manifest_parse_never_panics(s in ".{0,200}") {
        let _ = Manifest::from_str(&s);
    }
}

/// A valid manifest around generated versions, notes and numbers.
fn manifest_text(
    cigs: &[Version],
    cartons: &[Version],
    notes: &str,
    seq: u32,
    yank_idx: usize,
    adv_fixed: &Version,
) -> String {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let mut s = format!(
        "format = 1\nchannel = \"stable\"\nsequence = {seq}\ngenerated = 2026-11-01T00:00:00Z\nexpires = 2026-12-01T00:00:00Z\n\n\
         [tool.cig]\nrepo = \"o/cig\"\ntag = \"v{{version}}\"\nlayout = \"binary\"\nasset = \"cig-{{version}}-{{os}}-{{arch}}{{exe}}\"\n\
         shims = [ {{ name = \"cig\", path = \"bin/cig\" }} ]\ntargets = [ \"linux-x86_64\", \"windows-x86_64\" ]\n\n"
    );
    for v in cigs {
        s.push_str(&format!(
            "[build.cig.\"{v}\"]\nreleased = 2026-11-02\n[build.cig.\"{v}\".linux-x86_64]\nsha256 = \"{}\"\nsize = {}\n\
             [build.cig.\"{v}\".windows-x86_64]\nsha256 = \"{}\"\nsize = 7\nurl = \"https://x.invalid/{v}\"\n\n",
            "b".repeat(64), 100 + v.major, "c".repeat(64)
        ));
    }
    for (i, c) in cartons.iter().enumerate() {
        let pin = &cigs[i % cigs.len()];
        s.push_str(&format!(
            "[carton.\"{c}\"]\nreleased = 2026-11-0{}\nnotes = {}\ntested = [ \"suite-{i}\" ]\ncig = \"{pin}\"\n\
             [carton.\"{c}\".packs]\ndev = [ \"cig\" ]\n\n",
            1 + i % 9,
            q(notes)
        ));
    }
    for v in cigs {
        s.push_str(&format!(
            "[product.cig.\"{v}\"]\nembeds = {{ journal_format = {}, cig_runtime = \"{v}\" }}\nos_min = {{ linux = \"kernel 3.10\" }}\n\n",
            v.minor
        ));
    }
    let y = &cartons[yank_idx % cartons.len()];
    s.push_str(&format!(
        "[yanked]\n\"carton {y}\" = {{ reason = {}, fixed_in = \"{adv_fixed}\" }}\n\"cig {}\" = \"plain\"\n\n",
        q(&format!("why {notes}")),
        cigs[0]
    ));
    s.push_str(&format!(
        "[[advisory]]\nid = \"A-1\"\naffects = [ \"carton <{adv_fixed}\", \"cig >=1.0.0, <2.0.0\" ]\nfixed_in = \"{adv_fixed}\"\nseverity = \"bug\"\nsummary = {}\n",
        q(notes)
    ));
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn manifest_render_round_trip(
        cigs in proptest::collection::btree_set(version_strategy(), 1..5),
        cartons in proptest::collection::btree_set(version_strategy(), 1..5),
        notes in "[ -~]{0,24}",
        seq in 0u32..1_000_000,
        yank_idx in 0usize..8,
        fixed in version_strategy(),
    ) {
        let cigs: Vec<Version> = cigs.into_iter().collect();
        let cartons: Vec<Version> = cartons.into_iter().collect();
        let text = manifest_text(&cigs, &cartons, &notes, seq, yank_idx, &fixed);
        let m = Manifest::from_str(&text).map_err(|e| TestCaseError::fail(format!("{e}\n{text}")))?;
        prop_assert_eq!(m.cartons.len(), cartons.len());
        let rendered = m.render();
        let again = Manifest::from_str(&rendered).map_err(|e| TestCaseError::fail(format!("{e}\n{rendered}")))?;
        prop_assert_eq!(again, m);
    }
}
