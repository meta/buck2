/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! User settings read from a TOML file at startup: the palette, the
//! handful of view defaults that are a matter of taste rather than of
//! what the trace says, and where links out of the window go.
//!
//! ```toml
//! [appearance]
//! row_height = 12.0
//! [links]
//! # `{id}` is the build ID; "" turns the link off.
//! build = "https://example.com/builds/{id}"
//! ```
//!
//! Three rules hold this together.
//!
//! **Colours are keyed by name, never by position.** A stage's colour is
//! configured under its `STAGE_ORDER` key and a class's under its
//! `CLASS_NAMES` name, so inserting a stage cannot silently repaint the
//! ones after it — the failure mode a positional list would have.
//!
//! **The configurable key set is derived, not restated.** `[colors.stage]`
//! accepts exactly the keys in [`crate::detail::STAGE_ORDER`] and
//! `[colors.ui]` exactly those in [`UI_COLORS`], so adding a stage or a
//! window colour makes it configurable with no edit here.
//!
//! **Settings are process-wide and installed once.** Only a binary's
//! startup calls [`install_from_disk`]; nothing else reads the user's
//! home directory. Tests therefore see [`Settings::builtin`], which is
//! what keeps the snapshot suite from rendering a developer's personal
//! palette.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::detail::STAGE_COLOR;
use crate::detail::STAGE_N;
use crate::detail::STAGE_ORDER;
use crate::types::CLASS_NAMES;

/// Number of action classes, from the class-name table.
pub const CLASS_N: usize = CLASS_NAMES.len();

/// Every window colour that is neither a class nor a stage, with its
/// built-in value. One row here is the whole cost of making a new colour
/// configurable.
pub const UI_COLORS: &[(&str, [u8; 3])] = &[
    ("critical_path", [0xd0, 0x30, 0x30]),
    ("cache_outline", [0x1a, 0x7f, 0x37]),
    ("ancestry", [0x00, 0x9a, 0x9a]),
    ("failure", [0xe1, 0x57, 0x59]),
    ("unmeasured", [0xc4, 0xc4, 0xc4]),
    ("stage_tail", [0xe3, 0xe3, 0xe3]),
    ("gutter", [0xe9, 0xe7, 0xe4]),
    ("flash", [0xff, 0x20, 0x79]),
    ("focus", [0x10, 0x50, 0xd0]),
    ("caution", [0xd0, 0x8a, 0x1e]),
];

/// Number of entries in [`UI_COLORS`].
pub const UI_N: usize = UI_COLORS.len();

/// Indices into [`UI_COLORS`]. A unit test pins each one to its key, so a
/// reordering of the table cannot quietly swap two colours.
pub const UI_CRITICAL_PATH: usize = 0;
pub const UI_CACHE_OUTLINE: usize = 1;
pub const UI_ANCESTRY: usize = 2;
pub const UI_FAILURE: usize = 3;
pub const UI_UNMEASURED: usize = 4;
pub const UI_STAGE_TAIL: usize = 5;
pub const UI_GUTTER: usize = 6;
pub const UI_FLASH: usize = 7;
pub const UI_FOCUS: usize = 8;
/// Evidence limits and other non-failure warnings in analysis panels.
pub const UI_CAUTION: usize = 9;

/// The built-in timeline palette, indexed by `Act::color`
/// (`parse::classify`), parallel to [`CLASS_NAMES`].
pub const CLASS_COLORS: [[u8; 3]; CLASS_N] = [
    [0x5b, 0x7d, 0xa8],
    [0xb0, 0x7a, 0xa5],
    [0x6f, 0xa8, 0x5b],
    [0xd9, 0xb8, 0x4a],
    [0xa8, 0x8a, 0x6f],
    [0xb8, 0xb2, 0xaa],
    [0x8a, 0xd0, 0xe6],
];

// ------------------------------------------------------------- settings

/// The resolved palette: built-in values with any configured overrides
/// already applied, so no consumer has to know a config file exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub class: [[u8; 3]; CLASS_N],
    /// `None` is `execute_us`, painted in the action's own class colour.
    pub stage: [Option<[u8; 3]>; STAGE_N],
    pub ui: [[u8; 3]; UI_N],
}

impl Palette {
    fn builtin() -> Self {
        let mut ui = [[0u8; 3]; UI_N];
        for (i, (_, c)) in UI_COLORS.iter().enumerate() {
            ui[i] = *c;
        }
        Self {
            class: CLASS_COLORS,
            stage: STAGE_COLOR,
            ui,
        }
    }
}

/// Row-height limits, shared with the ctrl-scroll zoom
/// (`canvas::ROW_H_MIN`/`ROW_H_MAX` alias these). One pair of numbers, so
/// a height the file is allowed to ask for is always one the timeline can
/// actually hold.
pub const ROW_HEIGHT_MIN: f32 = 3.0;
pub const ROW_HEIGHT_MAX: f32 = 48.0;

/// View defaults. `None` means "the code's own default" — an absent key
/// and an explicitly-set one are different things, and collapsing them
/// would make it impossible to ever change a default.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    /// Timeline row height in points.
    pub row_height: Option<f32>,
    /// Show the concurrency strip.
    pub concurrency_strip: Option<bool>,
    /// Show the lane gutter.
    pub lane_gutter: Option<bool>,
}

/// Where links out of the window go.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Links {
    /// The page for a build: `{id}` is replaced with the build ID, and an
    /// empty string turns the link off. `None` is [`BUILTIN_BUILD_LINK`].
    pub build: Option<String>,
}

/// The built-in build page: the Buck UI in Meta's build, and none in the
/// open-source one, as buck2 itself does for its console link.
pub const BUILTIN_BUILD_LINK: Option<&str> = if cfg!(fbcode_build) {
    Some("https://www.internalfb.com/buck2/{id}")
} else {
    None
};

/// Behaviour defaults.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Behavior {
    /// Open detail popups as native OS windows rather than in-window
    /// panels.
    pub native_popups: Option<bool>,
}

/// Everything the config file can say, resolved.
#[derive(Clone, Debug)]
pub struct Settings {
    pub palette: Palette,
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub links: Links,
    /// The file these came from; `None` when nothing was read.
    pub source: Option<PathBuf>,
}

impl Settings {
    /// The compiled-in defaults: what every test and every user without a
    /// config file sees.
    pub fn builtin() -> Self {
        Self {
            palette: Palette::builtin(),
            appearance: Appearance::default(),
            behavior: Behavior::default(),
            links: Links::default(),
            source: None,
        }
    }
}

// ------------------------------------------------------------ the file

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileColors {
    #[serde(default)]
    class: BTreeMap<String, String>,
    #[serde(default)]
    stage: BTreeMap<String, String>,
    #[serde(default)]
    ui: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRoot {
    #[serde(default)]
    colors: FileColors,
    #[serde(default)]
    appearance: Appearance,
    #[serde(default)]
    behavior: Behavior,
    #[serde(default)]
    links: Links,
}

/// Where the config file lives, honouring `RETRACE_CONFIG` first and
/// otherwise the platform's own config directory (`~/.config/retrace/` on
/// Linux, `~/Library/Application Support/retrace/` on macOS,
/// `%APPDATA%\retrace\` on Windows).
pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RETRACE_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    Some(dirs::config_dir()?.join("retrace").join("config.toml"))
}

fn parse_hex(s: &str) -> anyhow::Result<[u8; 3]> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("`{s}` is not a colour: want `#rrggbb`");
    }
    let pair = |i: usize| u8::from_str_radix(&h[i..i + 2], 16);
    Ok([pair(0)?, pair(2)?, pair(4)?])
}

/// Apply one `key = "#rrggbb"` table onto `slot`, resolving keys through
/// `index`. Unknown keys and unparseable colours are reported and
/// skipped: a typo in a colour name must not cost the user every other
/// setting in the file.
fn apply<'a>(
    table: &BTreeMap<String, String>,
    section: &str,
    index: impl Fn(&str) -> Option<usize>,
    known: impl Fn() -> Vec<&'a str>,
    warnings: &mut Vec<String>,
    mut slot: impl FnMut(usize, [u8; 3]),
) {
    for (k, v) in table {
        let Some(i) = index(k) else {
            warnings.push(format!(
                "[colors.{section}]: no such key `{k}` (known: {})",
                known().join(", ")
            ));
            continue;
        };
        match parse_hex(v) {
            Ok(c) => slot(i, c),
            Err(e) => warnings.push(format!("[colors.{section}] {k}: {e}")),
        }
    }
}

/// Read and resolve `text` as a config file. Returns the settings plus
/// every complaint about the file's contents; a malformed file is an
/// error, a merely surprising one is a warning.
pub fn parse(text: &str) -> anyhow::Result<(Settings, Vec<String>)> {
    let root: FileRoot = toml::from_str(text)?;
    let mut s = Settings::builtin();
    let mut warnings = Vec::new();

    apply(
        &root.colors.class,
        "class",
        |k| CLASS_NAMES.iter().position(|n| *n == k),
        || CLASS_NAMES.to_vec(),
        &mut warnings,
        |i, c| s.palette.class[i] = c,
    );
    apply(
        &root.colors.stage,
        "stage",
        |k| STAGE_ORDER.iter().position(|n| *n == k),
        || STAGE_ORDER.to_vec(),
        &mut warnings,
        |i, c| s.palette.stage[i] = Some(c),
    );
    apply(
        &root.colors.ui,
        "ui",
        |k| UI_COLORS.iter().position(|(n, _)| *n == k),
        || UI_COLORS.iter().map(|(n, _)| *n).collect(),
        &mut warnings,
        |i, c| s.palette.ui[i] = c,
    );

    let mut appearance = root.appearance;
    if let Some(h) = appearance.row_height {
        if !(ROW_HEIGHT_MIN..=ROW_HEIGHT_MAX).contains(&h) {
            warnings.push(format!(
                "[appearance] row_height: {h} is outside \
                 {ROW_HEIGHT_MIN}..={ROW_HEIGHT_MAX}, ignoring"
            ));
            appearance.row_height = None;
        }
    }

    let behavior = root.behavior;

    let mut links = root.links;
    if let Some(t) = &links.build
        && !t.is_empty()
        && !t.contains("{id}")
    {
        warnings.push(format!(
            "[links] build: `{t}` has no `{{id}}` to put the build ID in, ignoring"
        ));
        links.build = None;
    }

    s.appearance = appearance;
    s.behavior = behavior;
    s.links = links;
    Ok((s, warnings))
}

// ------------------------------------------------------- the singleton

static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// The process-wide settings, defaulting to [`Settings::builtin`] for any
/// caller that never installed a file — which is every test.
pub fn get() -> &'static Settings {
    SETTINGS.get_or_init(Settings::builtin)
}

/// The resolved palette, for the common case.
pub fn palette() -> &'static Palette {
    &get().palette
}

/// The timeline colour for an action class, by `Act::color`.
pub fn class_color(i: usize) -> egui::Color32 {
    let [r, g, b] = palette().class[i.min(CLASS_N - 1)];
    egui::Color32::from_rgb(r, g, b)
}

/// The colour for a stage band. `None` is `execute_us`, which takes the
/// action's own class colour instead.
pub fn stage_color(i: usize) -> Option<egui::Color32> {
    palette().stage[i].map(|[r, g, b]| egui::Color32::from_rgb(r, g, b))
}

/// One of the named window colours, by its `UI_*` index.
pub fn ui_color(i: usize) -> egui::Color32 {
    let [r, g, b] = palette().ui[i];
    egui::Color32::from_rgb(r, g, b)
}

/// The page for build `id`, from `[links] build` or [`BUILTIN_BUILD_LINK`];
/// `None` when there is no link to offer.
pub fn build_link(id: &str) -> Option<String> {
    build_link_in(&get().links, id)
}

fn build_link_in(links: &Links, id: &str) -> Option<String> {
    let template = match &links.build {
        Some(t) => t.as_str(),
        None => BUILTIN_BUILD_LINK?,
    };
    (!template.is_empty()).then(|| template.replace("{id}", id))
}

/// Read the config file and install it. Returns what to tell the user:
/// an unreadable or malformed file degrades to the built-in defaults with
/// a complaint rather than refusing to start, because a colour typo is
/// not a reason to be unable to look at a build.
///
/// Call once, before anything reads [`get`]. A later call cannot displace
/// settings already in use, and says so.
pub fn install_from_disk() -> Vec<String> {
    let Some(path) = config_path() else {
        return Vec::new();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // No config file is the normal case, not a problem to report.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![format!("{}: {e}", path.display())],
    };
    let (mut settings, mut warnings) = match parse(&text) {
        Ok(x) => x,
        Err(e) => return vec![format!("{}: {e:#}", path.display())],
    };
    settings.source = Some(path);
    if SETTINGS.set(settings).is_err() {
        warnings.push("settings were already in use; config file ignored".to_owned());
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_build_link_replaces_the_builtin_one() {
        let (s, warnings) = parse("[links]\nbuild = \"https://ci.example/b/{id}\"\n").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            build_link_in(&s.links, "abc").as_deref(),
            Some("https://ci.example/b/abc")
        );
        let (off, _) = parse("[links]\nbuild = \"\"\n").unwrap();
        assert_eq!(
            build_link_in(&off.links, "abc"),
            None,
            "an empty template turns the link off"
        );
    }

    #[test]
    fn a_build_link_without_a_place_for_the_id_is_refused() {
        let (s, warnings) = parse("[links]\nbuild = \"https://ci.example/latest\"\n").unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(s.links.build, None, "falls back to the built-in link");
    }

    #[test]
    fn the_builtin_build_link_follows_the_build() {
        let link = build_link_in(&Links::default(), "abc");
        if cfg!(fbcode_build) {
            assert_eq!(
                link.as_deref(),
                Some("https://www.internalfb.com/buck2/abc")
            );
        } else {
            assert_eq!(
                link, None,
                "the open-source build has no build page to link to"
            );
        }
    }

    #[test]
    fn out_of_range_class_uses_the_final_palette_entry() {
        assert_eq!(class_color(usize::MAX), class_color(CLASS_N - 1));
    }

    #[test]
    fn ui_index_constants_name_their_own_rows() {
        // These indices are how call sites reach a colour, so a
        // reordering of UI_COLORS that left them behind would repaint the
        // window with no test failing anywhere else.
        for (i, key) in [
            (UI_CRITICAL_PATH, "critical_path"),
            (UI_CACHE_OUTLINE, "cache_outline"),
            (UI_ANCESTRY, "ancestry"),
            (UI_FAILURE, "failure"),
            (UI_UNMEASURED, "unmeasured"),
            (UI_STAGE_TAIL, "stage_tail"),
            (UI_GUTTER, "gutter"),
            (UI_FLASH, "flash"),
            (UI_FOCUS, "focus"),
            (UI_CAUTION, "caution"),
        ] {
            assert_eq!(UI_COLORS[i].0, key, "UI_COLORS[{i}] is not {key}");
        }
        assert_eq!(UI_COLORS.len(), UI_N);
    }

    #[test]
    fn colour_keys_are_unique() {
        // Two rows with one key would make the second unreachable: the
        // `position` lookups in `parse` always find the first.
        let mut keys: Vec<&str> = UI_COLORS.iter().map(|(n, _)| *n).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate key in UI_COLORS");
    }

    #[test]
    fn an_empty_file_is_the_built_in_palette() {
        let (s, w) = parse("").unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.palette, Palette::builtin());
        assert_eq!(s.appearance, Appearance::default());
    }

    #[test]
    fn colours_are_applied_by_name() {
        let (s, w) = parse(
            r##"
            [colors.class]
            binary = "#010203"
            [colors.stage]
            execute_us = "112233"
            cache_hit_us = "112233"
            prepare_us = "112233"
            cas_artifact_us = "112233"
            local_finalize_outputs_us = "112233"
            local_cache_check_us = "112233"
            [colors.ui]
            focus = "#ffffff"
            "##,
        )
        .unwrap();
        assert!(w.is_empty(), "{w:?}");
        let binary = CLASS_NAMES.iter().position(|n| *n == "binary").unwrap();
        assert_eq!(s.palette.class[binary], [0x01, 0x02, 0x03]);
        let exec = STAGE_ORDER.iter().position(|n| *n == "execute_us").unwrap();
        // `execute_us` is the one stage with no built-in colour; naming it
        // in the file is how a user opts out of the class colour.
        assert_eq!(s.palette.stage[exec], Some([0x11, 0x22, 0x33]));
        for key in [
            "cache_hit_us",
            "prepare_us",
            "cas_artifact_us",
            "local_finalize_outputs_us",
            "local_cache_check_us",
        ] {
            let stage = STAGE_ORDER.iter().position(|name| *name == key).unwrap();
            assert_eq!(s.palette.stage[stage], Some([0x11, 0x22, 0x33]));
        }
        assert_eq!(s.palette.ui[UI_FOCUS], [0xff, 0xff, 0xff]);
    }

    #[test]
    fn a_bad_colour_is_a_warning_and_costs_only_itself() {
        let (s, w) = parse(
            r##"
            [colors.class]
            binary = "not-a-colour"
            library = "#040506"
            "##,
        )
        .unwrap();
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("binary"), "{w:?}");
        let binary = CLASS_NAMES.iter().position(|n| *n == "binary").unwrap();
        let library = CLASS_NAMES.iter().position(|n| *n == "library").unwrap();
        assert_eq!(s.palette.class[binary], CLASS_COLORS[binary]);
        assert_eq!(s.palette.class[library], [0x04, 0x05, 0x06]);
    }

    #[test]
    fn an_unknown_colour_key_names_the_ones_that_exist() {
        let (_, w) = parse("[colors.stage]\nexecute = \"#000000\"\n").unwrap();
        assert_eq!(w.len(), 1, "{w:?}");
        // the misspelling has to be diagnosable from the message alone
        assert!(w[0].contains("execute_us"), "{w:?}");
    }

    #[test]
    fn a_misspelled_section_is_an_error_not_a_silent_no_op() {
        // Warning-and-continue is right for one bad key inside a known
        // table; a whole section nobody reads is a file that does nothing,
        // and saying so is the only way the user finds out.
        assert!(parse("[colours]\n").is_err());
        assert!(parse("[colors.klass]\n").is_err());
        assert!(parse("[appearance]\nrow_hight = 3\n").is_err());
    }

    #[test]
    fn out_of_range_scalars_are_refused_rather_than_obeyed() {
        let (s, w) = parse("[appearance]\nrow_height = 0.0\n").unwrap();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(s.appearance.row_height, None);

        assert!(parse("[behavior]\ndetail_layout = \"q\"\n").is_err());
    }

    #[test]
    fn scalars_survive_when_they_are_in_range() {
        let (s, w) = parse(
            "[appearance]\nrow_height = 22.5\nlane_gutter = false\n\
             [behavior]\nnative_popups = true\n",
        )
        .unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.appearance.row_height, Some(22.5));
        assert_eq!(s.appearance.lane_gutter, Some(false));
        assert_eq!(s.appearance.concurrency_strip, None);
        assert_eq!(s.behavior.native_popups, Some(true));
    }

    #[test]
    fn every_stage_is_configurable_by_its_own_key() {
        // The point of deriving the key set from the stage table: a stage
        // added there is configurable here with no edit to this module.
        for (i, key) in STAGE_ORDER.iter().enumerate() {
            let (s, w) = parse(&format!("[colors.stage]\n{key} = \"#0a0b0c\"\n")).unwrap();
            assert!(w.is_empty(), "{key}: {w:?}");
            assert_eq!(s.palette.stage[i], Some([0x0a, 0x0b, 0x0c]), "{key}");
        }
    }

    #[test]
    fn hex_accepts_both_spellings_and_rejects_the_rest() {
        assert_eq!(parse_hex("#aabbcc").unwrap(), [0xaa, 0xbb, 0xcc]);
        assert_eq!(parse_hex("AABBCC").unwrap(), [0xaa, 0xbb, 0xcc]);
        for bad in ["#abc", "#aabbccdd", "", "#gg0000", "0x00ff00"] {
            assert!(parse_hex(bad).is_err(), "{bad} should not parse");
        }
    }
}
