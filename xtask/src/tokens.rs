// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask tokens`: build everything that is derived from `design/tokens.json`
//! (design standard #31): `crates/cigi-ui/src/tokens.rs` and `design/generated/*`.
//! Nothing derived is ever edited by hand. `cargo xtask tokens --check` (and the
//! `generated_files_match_tokens` test) fail when a file on disk differs from a fresh build.

use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

const SOURCE: &str = "design/tokens.json";

/// One generated file: its path from the workspace root and its text.
pub struct Output {
    pub path: &'static str,
    pub text: String,
}

type R<T> = Result<T, String>;

/// Build every generated file from the text of `design/tokens.json`.
pub fn generate(src: &str) -> R<Vec<Output>> {
    let root: Value = serde_json::from_str(src).map_err(|e| format!("{SOURCE}: {e}"))?;
    let t = Tokens::read(&root)?;
    Ok(vec![
        Output {
            path: "crates/cigi-ui/src/tokens.rs",
            text: rust(&t),
        },
        Output {
            path: "design/generated/tokens.css",
            text: css(&t),
        },
        Output {
            path: "design/generated/egui-theme.rs",
            text: egui(&t),
        },
        Output {
            path: "design/generated/terminal-palettes.json",
            text: palettes(&t)?,
        },
        Output {
            path: "design/generated/svg-styles.css",
            text: svg(&t),
        },
        Output {
            path: "design/generated/badges.json",
            text: badges(&t)?,
        },
    ])
}

/// Write every generated file under `root`, or with `check` compare and say which differ.
pub fn run(root: &Path, check: bool) -> R<()> {
    let p = root.join(SOURCE);
    let src = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut stale = Vec::new();
    for o in generate(&src)? {
        let path = root.join(o.path);
        if check {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(o.text.as_str()) {
                stale.push(o.path);
            }
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, &o.text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    if stale.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "generated files are out of date with {SOURCE}: {}; run `cargo xtask tokens`",
            stale.join(", ")
        ))
    }
}

// ---- reading -----------------------------------------------------------------------------

type Rgb = (u8, u8, u8);

struct RoleDef {
    /// kebab-case, as in the JSON.
    name: String,
    /// dark, light and neutral; `None` for an attribute role (bold).
    colors: Option<(Rgb, Rgb, Rgb)>,
    ansi256: Option<(u8, u8, u8)>,
    /// The SGR parameters for 16 colors per theme (dark, light, neutral), or the attribute.
    sgr: [String; 3],
}

struct PairDef {
    name: String,
    fg: Rgb,
    bg: Rgb,
    min: f64,
}

struct Tokens {
    version: String,
    /// every flat colour: (name, hex), in file order.
    flat: Vec<(String, Rgb)>,
    /// every dark/light colour: (group.name, dark, light).
    themed: Vec<(String, Rgb, Rgb)>,
    ui: Vec<(String, Rgb, Rgb)>,
    roles: Vec<RoleDef>,
    bg: (Rgb, Rgb),
    pairs: Vec<PairDef>,
    badges: Vec<(String, Rgb)>,
    fonts: Vec<(String, String)>,
    sizes: Vec<(String, String)>,
    motion: Vec<(String, String)>,
    budgets: Vec<(String, u64)>,
}

fn get<'a>(v: &'a Value, path: &str) -> R<&'a Value> {
    path.split('.').try_fold(v, |cur, k| {
        cur.get(k)
            .ok_or_else(|| format!("{SOURCE}: nothing at `{path}`"))
    })
}

fn parse_hex(s: &str) -> R<Rgb> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6);
    let h = h.ok_or_else(|| format!("{SOURCE}: `{s}` is not #rrggbb"))?;
    let b = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("{SOURCE}: `{s}` is not hex"))
    };
    Ok((b(0)?, b(2)?, b(4)?))
}

/// A colour value: `#rrggbb` or an alias `{a.b.c}` followed to its end.
fn colour(root: &Value, v: &Value) -> R<Rgb> {
    let mut cur = v.clone();
    for _ in 0..8 {
        let raw = match cur.get("$value").unwrap_or(&cur) {
            Value::String(s) => s.clone(),
            other => return Err(format!("{SOURCE}: `{other}` is not a colour")),
        };
        match raw.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            Some(path) => cur = get(root, path)?.clone(),
            None => return parse_hex(&raw),
        }
    }
    Err(format!("{SOURCE}: alias loop at `{v}`"))
}

fn text(root: &Value, path: &str) -> R<String> {
    match get(root, path)?.get("$value") {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(format!("{SOURCE}: `{path}` has no string $value")),
    }
}

fn entries(v: &Value) -> impl Iterator<Item = (&String, &Value)> {
    v.as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| !k.starts_with('$'))
}

fn theme_pair(root: &Value, v: &Value) -> R<(Rgb, Rgb)> {
    let side = |k: &str| {
        v.get(k)
            .ok_or_else(|| format!("{SOURCE}: missing `{k}` in {v}"))
            .and_then(|x| colour(root, x))
    };
    Ok((side("dark")?, side("light")?))
}

fn srgb_distance(a: Rgb, b: Rgb) -> i32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The nearest entry of the xterm 256 palette (cube 16-231, greys 232-255). Ties go to the
/// higher index, which makes ember 208.
fn nearest_256(c: Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut best = (i32::MAX, 16u8);
    for i in 16u8..=255 {
        let rgb = if i < 232 {
            let n = usize::from(i - 16);
            (LEVELS[n / 36], LEVELS[n / 6 % 6], LEVELS[n % 6])
        } else {
            let g = 8 + 10 * (i - 232);
            (g, g, g)
        };
        let d = srgb_distance(c, rgb);
        if d <= best.0 {
            best = (d, i);
        }
    }
    best.1
}

impl Tokens {
    fn read(root: &Value) -> R<Tokens> {
        let version = text_plain(root, "version")?;
        let mut flat = Vec::new();
        let mut themed = Vec::new();
        for (k, v) in entries(get(root, "color")?) {
            if v.get("$value").is_some() {
                flat.push((k.clone(), colour(root, v)?));
            } else {
                let (d, l) = theme_pair(root, v)?;
                themed.push((k.clone(), d, l));
            }
        }
        let mut ui = Vec::new();
        for (k, v) in entries(get(root, "ui")?) {
            let (d, l) = theme_pair(root, v)?;
            ui.push((k.clone(), d, l));
        }
        let bg = theme_pair(root, get(root, "terminal.background")?)?;
        let mut roles = Vec::new();
        for (k, v) in entries(get(root, "terminal.role")?) {
            if let Some(attr) = v.get("attribute").and_then(Value::as_str) {
                roles.push(RoleDef {
                    name: k.clone(),
                    colors: None,
                    ansi256: None,
                    sgr: [attr.to_string(), attr.to_string(), attr.to_string()],
                });
                continue;
            }
            let (d, l) = theme_pair(root, v)?;
            let n = colour(
                root,
                v.get("neutral")
                    .ok_or_else(|| format!("{SOURCE}: role `{k}` has no neutral color"))?,
            )?;
            let sixteen = |theme: &str| -> R<String> {
                v.get("ansi16")
                    .and_then(|a| a.get(theme))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("{SOURCE}: role `{k}` has no ansi16.{theme}"))
            };
            roles.push(RoleDef {
                name: k.clone(),
                colors: Some((d, l, n)),
                ansi256: Some((nearest_256(d), nearest_256(l), nearest_256(n))),
                sgr: [sixteen("dark")?, sixteen("light")?, sixteen("neutral")?],
            });
        }
        let mut pairs = Vec::new();
        for p in get(root, "pairs.list")?.as_array().into_iter().flatten() {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .ok_or("pair without a name")?;
            let side = |k: &str| {
                p.get(k)
                    .ok_or_else(|| format!("{SOURCE}: pair `{name}` lacks `{k}`"))
                    .and_then(|x| colour(root, x))
            };
            pairs.push(PairDef {
                name: name.to_string(),
                fg: side("fg")?,
                bg: side("bg")?,
                min: p
                    .get("min")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| format!("{SOURCE}: pair `{name}` lacks `min`"))?,
            });
        }
        let mut badges = Vec::new();
        for (k, v) in entries(get(root, "badge")?) {
            badges.push((k.clone(), colour(root, v)?));
        }
        let strings = |group: &str| -> R<Vec<(String, String)>> {
            entries(get(root, group)?)
                .map(|(k, _)| Ok((k.clone(), text(root, &format!("{group}.{k}"))?)))
                .collect()
        };
        let mut budgets = Vec::new();
        for (k, v) in entries(get(root, "budget")?) {
            let ms = v
                .get("$value")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("{SOURCE}: budget `{k}` is not a number"))?;
            budgets.push((k.clone(), ms));
        }
        Ok(Tokens {
            version,
            flat,
            themed,
            ui,
            roles,
            bg,
            pairs,
            badges,
            fonts: strings("font")?,
            sizes: strings("size")?,
            motion: strings("motion")?,
            budgets,
        })
    }
}

fn text_plain(root: &Value, key: &str) -> R<String> {
    root.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{SOURCE}: no `{key}`"))
}

// ---- writing -----------------------------------------------------------------------------

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn hex_bare(c: Rgb) -> String {
    hex(c)[1..].to_string()
}

fn pascal(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn snake(kebab: &str) -> String {
    kebab.replace('-', "_")
}

fn upper_snake(kebab: &str) -> String {
    snake(kebab).to_ascii_uppercase()
}

const CC_HEADER_CSS: &str = "/* SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) */\n/* SPDX-License-Identifier: CC-BY-4.0 */\n/* @generated by `cargo xtask tokens` from design/tokens.json. Do not edit. */\n";
const CC_HEADER_RS: &str = "// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)\n// SPDX-License-Identifier: CC-BY-4.0\n// @generated by `cargo xtask tokens` from design/tokens.json. Do not edit.\n";

fn rgb_lit(c: Rgb) -> String {
    format!("({:#04x}, {:#04x}, {:#04x})", c.0, c.1, c.2)
}

fn rust(t: &Tokens) -> String {
    let mut o = String::new();
    o.push_str("// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)\n// SPDX-License-Identifier: FSL-1.1-ALv2\n");
    o.push_str("// @generated by `cargo xtask tokens` from design/tokens.json. Do not edit.\n\n");
    let _ = writeln!(
        o,
        "//! The family palette as roles (design standard {}, issues #31 and #32). Every value comes\n//! from `design/tokens.json`; a test fails when this file and that one drift apart.\n//! Roles, never raw colour codes: the painter in `paint.rs` takes a `Role`.\n",
        t.version
    );
    let _ = writeln!(
        o,
        "/// The version of the design standard these tokens belong to.\npub const STANDARD_VERSION: &str = \"{}\";\n",
        t.version
    );
    o.push_str("/// Which background the colors are chosen for.\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Theme {\n    Dark,\n    Light,\n    /// The background is not known: colors that read on both black and white.\n    Neutral,\n}\n\n");
    o.push_str("/// Roles, never raw colour codes.\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n#[rustfmt::skip]\npub enum Role {\n");
    for r in &t.roles {
        let _ = writeln!(o, "    {},", pascal(&r.name));
    }
    o.push_str("}\n\n");
    o.push_str("/// Every role, in the order of the tokens file.\n#[rustfmt::skip]\npub const ROLES: &[Role] = &[\n");
    for r in &t.roles {
        let _ = writeln!(o, "    Role::{},", pascal(&r.name));
    }
    o.push_str("];\n\n");
    o.push_str("impl Role {\n    /// The role's name in `design/tokens.json`.\n    #[rustfmt::skip]\n    pub fn name(self) -> &'static str {\n        match self {\n");
    for r in &t.roles {
        let _ = writeln!(
            o,
            "            Role::{} => \"{}\",",
            pascal(&r.name),
            r.name
        );
    }
    o.push_str("        }\n    }\n}\n\n");
    o.push_str("/// One role's value at each depth.\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct Token {\n    pub rgb: (u8, u8, u8),\n    pub ansi256: u8,\n    /// SGR parameters for the 16-color fallback.\n    pub ansi16: &'static str,\n}\n\n");
    o.push_str("const fn t(r: u8, g: u8, b: u8, ansi256: u8, ansi16: &'static str) -> Token {\n    Token {\n        rgb: (r, g, b),\n        ansi256,\n        ansi16,\n    }\n}\n\n");
    o.push_str("/// The token for a role on a theme. An attribute role (`Bold`) has no colour: its token is\n/// unused and its `ansi16` is the attribute.\n#[rustfmt::skip]\npub fn token(role: Role, theme: Theme) -> Token {\n    match (theme, role) {\n");
    for r in &t.roles {
        let p = pascal(&r.name);
        match (r.colors, r.ansi256) {
            (Some((d, l, n)), Some((d6, l6, n6))) => {
                for (theme, c, c6, sgr) in [
                    ("Dark", d, d6, &r.sgr[0]),
                    ("Light", l, l6, &r.sgr[1]),
                    ("Neutral", n, n6, &r.sgr[2]),
                ] {
                    let _ = writeln!(
                        o,
                        "        (Theme::{theme}, Role::{p}) => t({:#04x}, {:#04x}, {:#04x}, {c6}, \"{sgr}\"),",
                        c.0, c.1, c.2
                    );
                }
            }
            _ => {
                let _ = writeln!(
                    o,
                    "        (_, Role::{p}) => t(0, 0, 0, 0, \"{}\"),",
                    r.sgr[0]
                );
            }
        }
    }
    o.push_str("    }\n}\n\n");
    let _ = writeln!(
        o,
        "/// The dark terminal background the contrast checks assume.\npub const BACKGROUND_DARK: (u8, u8, u8) = {};\n/// The light terminal background the contrast checks assume.\npub const BACKGROUND_LIGHT: (u8, u8, u8) = {};\n",
        rgb_lit(t.bg.0),
        rgb_lit(t.bg.1)
    );
    o.push_str("/// A foreground and background a component draws, with the WCAG ratio it has to reach.\n#[derive(Debug, Clone, Copy, PartialEq)]\npub struct Pair {\n    pub name: &'static str,\n    pub fg: (u8, u8, u8),\n    pub bg: (u8, u8, u8),\n    pub min: f64,\n}\n\n");
    o.push_str("/// Every pair the components use (`pairs.list` in the tokens file).\n#[rustfmt::skip]\npub const PAIRS: &[Pair] = &[\n");
    for p in &t.pairs {
        let _ = writeln!(
            o,
            "    Pair {{ name: \"{}\", fg: {}, bg: {}, min: {:?} }},",
            p.name,
            rgb_lit(p.fg),
            rgb_lit(p.bg),
            p.min
        );
    }
    o.push_str("];\n\n/// The terminal budgets of #32, in milliseconds.\npub mod budget {\n");
    for (k, ms) in &t.budgets {
        let _ = writeln!(o, "    pub const {}_MS: u64 = {ms};", upper_snake(k));
    }
    o.push_str("}\n");
    o
}

fn css(t: &Tokens) -> String {
    let mut o = String::from(CC_HEADER_CSS);
    let _ = writeln!(o, "/* design standard {} */", t.version);
    o.push_str(":root {\n");
    for (k, c) in &t.flat {
        let _ = writeln!(o, "  --cig-{k}: {};", hex(*c));
    }
    for (k, d, _) in &t.themed {
        let _ = writeln!(o, "  --cig-{k}: {};", hex(*d));
    }
    for (k, d, _) in &t.ui {
        let _ = writeln!(o, "  --cig-ui-{k}: {};", hex(*d));
    }
    for r in &t.roles {
        if let Some((d, _, _)) = r.colors {
            let _ = writeln!(o, "  --cig-role-{}: {};", r.name, hex(d));
        }
    }
    for (k, v) in &t.fonts {
        let _ = writeln!(o, "  --cig-font-{k}: {v};");
    }
    for (k, v) in &t.sizes {
        let _ = writeln!(o, "  --cig-{k}: {v};");
    }
    for (k, v) in &t.motion {
        let _ = writeln!(o, "  --cig-{k}: {v};");
    }
    o.push_str("}\n\n");
    let light = |o: &mut String, indent: &str| {
        for (k, _, l) in &t.themed {
            let _ = writeln!(o, "{indent}--cig-{k}: {};", hex(*l));
        }
        for (k, _, l) in &t.ui {
            let _ = writeln!(o, "{indent}--cig-ui-{k}: {};", hex(*l));
        }
        for r in &t.roles {
            if let Some((_, l, _)) = r.colors {
                let _ = writeln!(o, "{indent}--cig-role-{}: {};", r.name, hex(l));
            }
        }
    };
    o.push_str(":root[data-theme=\"light\"] {\n");
    light(&mut o, "  ");
    o.push_str(
        "}\n\n@media (prefers-color-scheme: light) {\n  :root:not([data-theme=\"dark\"]) {\n",
    );
    light(&mut o, "    ");
    o.push_str("  }\n}\n\n@media (prefers-reduced-motion: reduce) {\n  :root {\n    --cig-ember-breathe: 0ms;\n  }\n}\n");
    o
}

fn egui(t: &Tokens) -> String {
    let mut o = String::from(CC_HEADER_RS);
    let _ = writeln!(
        o,
        "// The family theme for egui windows (the installer's window, CigCompile's window).\n// design standard {}. Wrap a triple in `egui::Color32::from_rgb(r, g, b)`.\n",
        t.version
    );
    o.push_str("pub struct Palette {\n");
    for (k, _, _) in &t.ui {
        let _ = writeln!(o, "    pub {}: [u8; 3],", snake(k));
    }
    for r in t.roles.iter().filter(|r| r.colors.is_some()) {
        let _ = writeln!(o, "    pub {}: [u8; 3],", snake(&r.name));
    }
    o.push_str("}\n\n");
    for (label, dark) in [("DARK", true), ("LIGHT", false)] {
        let pick = |p: (Rgb, Rgb)| if dark { p.0 } else { p.1 };
        let _ = writeln!(o, "pub const {label}: Palette = Palette {{");
        for (k, d, l) in &t.ui {
            let c = pick((*d, *l));
            let _ = writeln!(o, "    {}: [{}, {}, {}],", snake(k), c.0, c.1, c.2);
        }
        for r in &t.roles {
            if let Some(p) = r.colors {
                let c = pick((p.0, p.1));
                let _ = writeln!(o, "    {}: [{}, {}, {}],", snake(&r.name), c.0, c.1, c.2);
            }
        }
        o.push_str("};\n\n");
    }
    let _ = writeln!(
        o,
        "/// The focus ring is {} wide, in `ember-bright` on dark and `ember-ink` on light.",
        t.sizes
            .iter()
            .find(|(k, _)| k == "focus-ring")
            .map_or("2px", |(_, v)| v.as_str())
    );
    let ring = |name: &str, dark: bool| {
        t.roles
            .iter()
            .find(|r| r.name == name)
            .and_then(|r| r.colors)
            .map_or((0, 0, 0), |c| if dark { c.0 } else { c.1 })
    };
    let (d, l) = (ring("ember-bright", true), ring("ember-ink", false));
    let _ = writeln!(
        o,
        "pub const FOCUS_RING_DARK: [u8; 3] = [{}, {}, {}];",
        d.0, d.1, d.2
    );
    let _ = writeln!(
        o,
        "pub const FOCUS_RING_LIGHT: [u8; 3] = [{}, {}, {}];",
        l.0, l.1, l.2
    );
    o
}

fn palettes(t: &Tokens) -> R<String> {
    let mut root = serde_json::Map::new();
    root.insert("version".into(), Value::String(t.version.clone()));
    for (depth, key) in [("truecolor", 0), ("ansi256", 1), ("ansi16", 2)] {
        let mut themes = serde_json::Map::new();
        for (theme, at) in [("dark", 0), ("light", 1), ("neutral", 2)] {
            let mut roles = serde_json::Map::new();
            for r in &t.roles {
                let v = match (r.colors, r.ansi256, key) {
                    (Some(c), _, 0) => Value::String(hex([c.0, c.1, c.2][at])),
                    (Some(_), Some(c), 1) => Value::from([c.0, c.1, c.2][at]),
                    (_, _, 2) => Value::String(r.sgr[at].clone()),
                    _ => continue,
                };
                roles.insert(r.name.clone(), v);
            }
            themes.insert(theme.into(), Value::Object(roles));
        }
        root.insert(depth.into(), Value::Object(themes));
    }
    serde_json::to_string_pretty(&Value::Object(root))
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}

fn svg(t: &Tokens) -> String {
    let c = |name: &str| {
        t.flat
            .iter()
            .find(|(k, _)| k == name)
            .map_or_else(String::new, |(_, v)| hex(*v))
    };
    let mut o = String::from(CC_HEADER_CSS);
    o.push_str("/* Paste inside <style> in an SVG diagram. Diagrams are 820 px wide. */\n");
    let font = t
        .fonts
        .iter()
        .find(|(k, _)| k == "mono")
        .map_or("monospace", |(_, v)| v.as_str());
    let _ = writeln!(o, "text {{ font-family: {font}; fill: {}; }}", c("ash-400"));
    let _ = writeln!(o, ".label {{ fill: {}; font-size: 14px; }}", c("ash-300"));
    let _ = writeln!(o, ".muted {{ fill: {}; font-size: 12px; }}", c("ash-500"));
    let _ = writeln!(o, ".ember {{ fill: {}; }}", c("ember"));
    let _ = writeln!(
        o,
        ".box {{ fill: {}; stroke: {}; stroke-width: 1; rx: 6; }}",
        c("night-800"),
        c("ash-400")
    );
    let _ = writeln!(
        o,
        ".arrow {{ fill: none; stroke: {}; stroke-width: 1.5; stroke-linecap: round; }}",
        c("ember")
    );
    let _ = writeln!(o, ".filter {{ fill: {}; }}", c("filter"));
    let _ = writeln!(o, ".bg {{ fill: {}; }}", c("night-900"));
    o
}

fn badges(t: &Tokens) -> R<String> {
    let mut m = serde_json::Map::new();
    for (k, c) in &t.badges {
        m.insert(k.clone(), Value::String(hex_bare(*c)));
    }
    serde_json::to_string_pretty(&Value::Object(m))
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}
