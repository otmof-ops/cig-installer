// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The message catalogue (#31, "the voice, and plain beside it"): every user-facing sentence
//! the UI owns is written here in both forms at once, side by side. Neither form is derived
//! from the other; a writer sees both while writing.
//!
//! * The voice is warm, Australian and a bit cheeky, and goes straight on loss, security and
//!   money.
//! * Plain says the same facts in neutral words. It is automatic under `CI`, a screen reader,
//!   `--plain` and `voice = plain`, and `--json` carries data instead of either.
//!
//! `{0}`, `{1}` ... are filled in by [`Msg::render`]. Both forms must use the same
//! placeholders; `every_message_has_both_forms` (tests/catalogue.rs) checks that, and the
//! rules of design/voice.md (no exclamation marks, no "simply", "just" or "easy").

/// One message in both forms.
#[derive(Debug, Clone, Copy)]
pub struct Msg {
    pub id: &'static str,
    pub voice: &'static str,
    pub plain: &'static str,
}

impl Msg {
    /// The text for a mode, with `{0}`, `{1}` ... replaced by `args`.
    pub fn render(&self, plain: bool, args: &[&str]) -> String {
        let mut out = if plain { self.plain } else { self.voice }.to_string();
        for (i, a) in args.iter().enumerate() {
            out = out.replace(&format!("{{{i}}}"), a);
        }
        out
    }
}

pub const NO_TERMINAL: Msg = Msg {
    id: "ui.no-terminal",
    voice: "nobody's at the keyboard to ask: took \"{0}\" for \"{1}\" ({2})",
    plain: "no terminal to ask on: took \"{0}\" for \"{1}\" ({2})",
};

pub const FLAG_ANSWERED: Msg = Msg {
    id: "ui.flag-answered",
    voice: "--yes: said {0} to \"{1}\"",
    plain: "--yes: {0} to \"{1}\"",
};

pub const WHY_PASS_YES: Msg = Msg {
    id: "ui.why-pass-yes",
    voice: "pass --yes to say yes for next time",
    plain: "pass --yes to say yes",
};

pub const WHY_IRREVERSIBLE: Msg = Msg {
    id: "ui.why-irreversible",
    voice: "this one can't be undone, so --yes alone won't do it: pass --yes-irreversible",
    plain: "this step is irreversible: pass --yes-irreversible to say yes",
};

pub const ENTER_DEFAULT: Msg = Msg {
    id: "ui.enter-default",
    voice: "(enter = {0})",
    plain: "(enter = {0})",
};

pub const ENTER_DEFAULT_IRREVERSIBLE: Msg = Msg {
    id: "ui.enter-default-irreversible",
    voice: "(enter = {0}; this can't be undone)",
    plain: "(enter = {0}; this cannot be undone)",
};

pub const PICK_ONE: Msg = Msg {
    id: "ui.pick-one",
    voice: "that's not one of them. pick one of: {0}",
    plain: "not an option. pick one of: {0}",
};

pub const CHOICES_DEFAULT: Msg = Msg {
    id: "ui.choices-default",
    voice: "default",
    plain: "default",
};

/// The four kinds of no, in both forms. The voice forms are the family's catchphrases and
/// must equal `hear` in `errors/registry.toml` (a test compares them).
pub const NO_CANT_SEE_ANY: Msg = Msg {
    id: "no.cant-see-any",
    voice: "can't see any ciggies bro",
    plain: "not found",
};

pub const NO_THESE_ARE_MINE: Msg = Msg {
    id: "no.these-are-mine",
    voice: "these are MY ciggies",
    plain: "not allowed",
};

pub const NO_DONT_HAVE_ANY: Msg = Msg {
    id: "no.dont-have-any",
    voice: "don't have any over here",
    plain: "none available",
};

pub const NO_NEVER_HEARD_OF_YOU: Msg = Msg {
    id: "no.never-heard-of-you",
    voice: "never heard of you",
    plain: "not authenticated",
};

/// Every message above, for the catalogue tests.
pub const ALL: &[Msg] = &[
    NO_TERMINAL,
    FLAG_ANSWERED,
    WHY_PASS_YES,
    WHY_IRREVERSIBLE,
    ENTER_DEFAULT,
    ENTER_DEFAULT_IRREVERSIBLE,
    PICK_ONE,
    CHOICES_DEFAULT,
    NO_CANT_SEE_ANY,
    NO_THESE_ARE_MINE,
    NO_DONT_HAVE_ANY,
    NO_NEVER_HEARD_OF_YOU,
];

/// The message for one of the four kinds of no (`none` and `depends` have none).
pub fn kind_of_no(id: &str) -> Option<&'static Msg> {
    match id {
        "cant-see-any" => Some(&NO_CANT_SEE_ANY),
        "these-are-mine" => Some(&NO_THESE_ARE_MINE),
        "dont-have-any" => Some(&NO_DONT_HAVE_ANY),
        "never-heard-of-you" => Some(&NO_NEVER_HEARD_OF_YOU),
        _ => None,
    }
}
