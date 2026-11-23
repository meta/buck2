/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The settings reference `buck2 docs settings` prints: every section and
//! key, from the declarations alone.

use std::fmt::Write;

use serde_json::Value;

use crate::metadata::OverrideSource;
use crate::metadata::SectionMetadata;
use crate::metadata::SectionScope;
use crate::metadata::SettingKeyMetadata;
use crate::settings::ALL_SECTION_METADATA;

fn escape_cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

/// The default as it would be written in a settings file. Both builds are
/// shown when they differ: the reference is published once for both.
fn default_cell(key: &SettingKeyMetadata) -> String {
    let show = |value: &Option<Value>| match value {
        Some(value) => format!("`{}`", escape_cell(&value.to_string())),
        None => "unset".to_owned(),
    };
    let oss = (key.oss_default)();
    let internal = (key.internal_default)();
    if oss == internal {
        return oss.map_or(String::new(), |value| show(&Some(value)));
    }
    format!(
        "{}; {} in Meta-internal builds",
        show(&oss),
        show(&internal)
    )
}

fn overridable_cell(key: &SettingKeyMetadata) -> String {
    if key.overridable_in.is_empty() {
        return "no".to_owned();
    }
    key.overridable_in
        .iter()
        .map(|source| match source {
            OverrideSource::CommandLine => "`--setting`",
            OverrideSource::LocalSettings => "local files",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn description_cell(key: &SettingKeyMetadata) -> String {
    let mut description = escape_cell(key.doc);
    if let Some(fallback) = key.buckconfig_fallback {
        let builds = if fallback.oss_only {
            "In open source builds, falls"
        } else {
            "Falls"
        };
        write!(
            description,
            " {builds} back to `{}` in `.buckconfig`.",
            fallback.key
        )
        .expect("writing to a String cannot fail");
    }
    description
}

fn write_section(out: &mut String, section: &SectionMetadata) {
    let scope = match section.scope {
        SectionScope::Daemon => "daemon",
        SectionScope::Invocation => "invocation",
        SectionScope::Dice => "dice",
    };
    write!(
        out,
        "\n## [{}]\n\n{}\n\nScope: `{scope}`.\n\n\
         | Key | Type | Default | Overridable | Description |\n\
         | --- | --- | --- | --- | --- |\n",
        section.section_name, section.doc,
    )
    .expect("writing to a String cannot fail");
    for key in section.settings {
        writeln!(
            out,
            "| `{}` | `{}` | {} | {} | {} |",
            key.name,
            escape_cell(key.type_name).replace(' ', ""),
            default_cell(key),
            overridable_cell(key),
            description_cell(key),
        )
        .expect("writing to a String cannot fail");
    }
}

fn render_sections(sections: &[SectionMetadata]) -> String {
    let mut sections: Vec<&SectionMetadata> = sections.iter().collect();
    sections.sort_by_key(|section| section.section_name);
    let mut out = String::from(
        "A section's scope says how long its values live: a change to a `daemon` setting starts \
         a new daemon, an `invocation` setting is read by every command, and a `dice` setting is \
         read by every command and a change to it recomputes what read it. The Overridable \
         column lists the sources that may set a key besides the repository's \
         `.bucksettings.toml`: `--setting` on the command line, the `.bucksettings.local.toml` \
         files, or neither. Values are written as TOML, so strings are quoted.\n",
    );
    for section in sections {
        write_section(&mut out, section);
    }
    out
}

/// Every Buck setting as Markdown, grouped by section.
pub fn render_markdown() -> String {
    render_sections(ALL_SECTION_METADATA)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::BuckconfigFallback;

    fn unset() -> Option<Value> {
        None
    }

    fn on() -> Option<Value> {
        Some(Value::Bool(true))
    }

    fn off() -> Option<Value> {
        Some(Value::Bool(false))
    }

    fn text() -> Option<Value> {
        Some(Value::String("sqlite".to_owned()))
    }

    #[test]
    fn test_render_sections() {
        static KEYS: &[SettingKeyMetadata] = &[
            SettingKeyMetadata {
                name: "backend",
                doc: "Where values go:\n`sqlite` | `noop`.",
                type_name: "Backend",
                overridable_in: &[OverrideSource::CommandLine, OverrideSource::LocalSettings],
                internal_default: text,
                oss_default: text,
                buckconfig_fallback: None,
            },
            SettingKeyMetadata {
                name: "upload",
                doc: "Whether logs are uploaded.",
                type_name: "bool",
                overridable_in: &[],
                internal_default: on,
                oss_default: off,
                buckconfig_fallback: Some(BuckconfigFallback {
                    key: "buck2.upload",
                    oss_only: false,
                }),
            },
            SettingKeyMetadata {
                name: "url",
                doc: "Where logs are uploaded.",
                type_name: "String",
                overridable_in: &[OverrideSource::LocalSettings],
                internal_default: unset,
                oss_default: unset,
                buckconfig_fallback: Some(BuckconfigFallback {
                    key: "buck2.url",
                    oss_only: true,
                }),
            },
        ];
        let sections = [
            SectionMetadata {
                section_name: "zeta",
                section_version: 0,
                scope: SectionScope::Dice,
                doc: "Sorted after `alpha`.",
                settings: &[],
            },
            SectionMetadata {
                section_name: "alpha",
                section_version: 3,
                scope: SectionScope::Daemon,
                doc: "Logs.",
                settings: KEYS,
            },
        ];
        let expected = "
## [alpha]

Logs.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `backend` | `Backend` | `\"sqlite\"` | `--setting`, local files | Where values go: `sqlite` \\| `noop`. |
| `upload` | `bool` | `false`; `true` in Meta-internal builds | no | Whether logs are uploaded. Falls back to `buck2.upload` in `.buckconfig`. |
| `url` | `String` |  | local files | Where logs are uploaded. In open source builds, falls back to `buck2.url` in `.buckconfig`. |

## [zeta]

Sorted after `alpha`.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
";
        let rendered = render_sections(&sections);
        let body = rendered
            .split_once('\n')
            .expect("the legend is followed by the sections")
            .1;
        assert_eq!(expected, body);
    }

    /// The reference is published as MDX, where a Rust doc link is not a link and `<` or `{`
    /// outside of code is a syntax error.
    #[test]
    fn test_docs_are_publishable() {
        fn check(what: &str, doc: &str) {
            assert!(!doc.trim().is_empty(), "{what} needs a doc comment");
            assert!(
                !doc.contains("[`"),
                "the doc comment of {what} has a Rust doc link, which the settings reference \
                 cannot resolve; name the setting or its values in backticks instead"
            );
            let prose: String = doc.split('`').step_by(2).collect();
            assert!(
                !prose.contains(['<', '{']),
                "the doc comment of {what} has `<` or `{{` outside of backticks"
            );
        }
        for section in ALL_SECTION_METADATA {
            check(&format!("`[{}]`", section.section_name), section.doc);
            for key in section.settings {
                check(&format!("`{}.{}`", section.section_name, key.name), key.doc);
            }
        }
    }

    #[test]
    fn test_every_section_is_rendered() {
        let rendered = render_markdown();
        for section in ALL_SECTION_METADATA {
            assert!(rendered.contains(&format!("\n## [{}]\n", section.section_name)));
            for key in section.settings {
                assert!(rendered.contains(&format!("\n| `{}` |", key.name)));
            }
        }
    }
}
