// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! A hand-run check of the two things that touch a real terminal: the background query
//! (OSC 11) and the one-keypress answer. `cargo run -p cigi-ui --example probe` prints the
//! theme it found and the time the query took, asks one question, and prints the answer.
//! The test suite never does either, so this is how they are tried on a real terminal.

use cigi_ui::{GlobalFlags, Question, Ui};
use std::time::Instant;

fn main() {
    let t = Instant::now();
    let ui = Ui::from_env_and_flags(&GlobalFlags::default());
    println!(
        "theme {:?}, colour {:?}, ascii {}, found in {} ms",
        ui.theme,
        ui.color,
        ui.ascii,
        t.elapsed().as_millis()
    );
    let q = Question::yes_no("press a key", true).summary("y, n or enter");
    let a = ui.ask(&q);
    println!("answered {:?} by {:?}", a.key, a.how);
}
