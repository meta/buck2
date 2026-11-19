/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;

use buck2_client_ctx::exit_result::ExitResult;
use clap::Command;
use clap::builder::PossibleValue;
use regex::Regex;

/// Common option headings that appear across multiple buck2 commands
const COMMON_OPTION_HEADINGS: &[&str] = &[
    "Universal Options",
    "Event Log Options",
    "Buckconfig Options",
    "Console Options",
];

#[derive(Debug, clap::Parser)]
#[clap(
    name = "markdown-help-doc",
    about = "Print out the buck2 subcommand markdown docs"
)]
pub(crate) struct MarkdownHelpDocCommand {
    #[clap(
        name = "SUB_COMMAND",
        help = "The buck2 sub command we want to generate the markdown doc"
    )]
    sub_cmd: String,
}

impl MarkdownHelpDocCommand {
    pub(crate) fn exec(self, top_level_cmd: clap::Command) -> ExitResult {
        if self.sub_cmd == "common-options" {
            let markdown = generate_common_options_doc(&top_level_cmd);
            return ExitResult::success().with_stdout(markdown.into_bytes());
        }

        // Handle "query" as an alias for "uquery"
        if self.sub_cmd == "query" {
            let markdown = generate_query_redirect_page();
            return ExitResult::success().with_stdout(markdown.into_bytes());
        }

        for subcommand in top_level_cmd.get_subcommands() {
            if subcommand.get_name() == self.sub_cmd {
                let markdown = cmd_markdown_help(subcommand);
                return ExitResult::success().with_stdout(markdown.into_bytes());
            }
        }
        ExitResult::bail(format!("Subcommand `{}` not found", self.sub_cmd))
    }
}

// ===================== Generate Markdown Docs =====================

fn cmd_markdown_help(cmd: &Command) -> String {
    let header = cmd_header_markdown(cmd);
    let content = cmd_content_markdown(cmd, vec!["buck2".to_owned()]).unwrap();
    format!("{header}\n{content}")
}

fn generate_query_redirect_page() -> String {
    let markdown = r#"# query

`query` is an alias for the `uquery` command.

Please see the [uquery](../uquery) documentation for details.
"#;
    markdown.to_owned()
}

fn generate_common_options_doc(cmd: &Command) -> String {
    let mut markdown = String::new();

    // Add a header
    writeln!(markdown, "# Common Options").unwrap();
    writeln!(markdown, "\nThis document provides an overview of common options that are available across multiple buck2 commands.\n").unwrap();

    // Collect options from the top-level command
    for section_name in COMMON_OPTION_HEADINGS {
        if let Some(section_content) = extract_section_options(cmd, section_name) {
            writeln!(markdown, "## {}\n", section_name).unwrap();
            markdown.push_str(&section_content);
            markdown.push('\n');
        }
    }

    // If we couldn't find options in the top-level command, search in subcommands
    // This is needed because some options might only appear in specific subcommands
    for section_name in COMMON_OPTION_HEADINGS {
        if markdown.contains(&format!("## {}", section_name)) {
            continue; // Already found this section
        }

        for subcommand in cmd.get_subcommands() {
            if let Some(section_content) = extract_section_options(subcommand, section_name) {
                writeln!(markdown, "## {}\n", section_name).unwrap();
                markdown.push_str(&section_content);
                markdown.push('\n');
                break;
            }
        }
    }

    markdown
}

fn extract_section_options(cmd: &Command, section_heading: &str) -> Option<String> {
    let mut section_options = vec![];

    // Get all arguments (both positional and options)
    for arg in cmd.get_arguments() {
        // Check if this argument belongs to the target help heading
        if let Some(help_heading) = arg.get_help_heading() {
            if help_heading == section_heading && !arg.is_hide_set() {
                section_options.push(arg);
            }
        }
    }

    if section_options.is_empty() {
        return None;
    }

    let mut output = String::new();
    for arg in section_options {
        write_arg_markdown(&mut output, arg).ok()?;
    }

    Some(output)
}

fn cmd_header_markdown(cmd: &clap::Command) -> String {
    let name = cmd.get_name();

    let contents = if name == "install" {
        "\
The `buck2 install` command builds an installable target, typically a mobile app (.apk or .app bundle), and installs it using an installer server to some location, typically an emulator/simulator or external device.

## How buck2 install works

The `InstallInfo` provider is used to make targets installable, it specifies an installer implementation (e.g. Android or Apple installer) and a set of files to install. For example (from `buck2 audit providers`):
```python
InstallInfo(
    installer = buck//src/com/facebook/buck/installer/apple:apple_installer,
    files = {
        \"app_bundle;\": <build artifact HelloWorldBundle.app>,
        \"options\": <build artifact install_apple_data.json>
    }
)
```

Buck connects to the installer using GRPC and sends individual files to install once they have finished building.

# Exopackage

For Android apks, buck install supports a feature to speed up iterative development called Exopackage. An _exopackage_ is a small shell of an Android app that contains the minimal code and resources needed to bootstrap loading the code for a full-fledged Android application. Loading the application code at runtime avoids a full reinstall of the app when testing typical code changes, which dramatically reduces the length of edit/refresh cycles.
".to_owned()
    } else {
        format!(
            "This document provides an overview of the commands and options available under `buck2 {name}`."
        )
    };

    format!(
        "\
# {name}

{contents}

"
    )
}

fn cmd_content_markdown(
    cmd: &clap::Command,
    mut path: Vec<String>,
) -> Result<String, std::fmt::Error> {
    let prefix = path.join(" ");
    let mut markdown = String::new();

    let about = cmd.get_long_about().or_else(|| cmd.get_about());
    let about = about.map_or("".to_owned(), |about| {
        escape_angle_brackets_for_mdx(&about.to_string())
    });

    // The text is written out directly rather than through a clap help template, which would
    // treat every `{...}` in the help text as a template tag.
    let name = cmd.get_display_name().unwrap_or_else(|| cmd.get_name());
    let usage = cmd.clone().render_usage().to_string();
    let usage = usage.strip_prefix("Usage: ").unwrap_or(&usage).to_owned();

    // header
    writeln!(
        markdown,
        "\
## `{prefix} {name}`

{about}

**Usage**: `{prefix} {usage}`
"
    )?;

    // subcommands
    let subcommands = cmd.get_subcommands().collect::<Vec<_>>();
    if !subcommands.is_empty() {
        writeln!(markdown, "### Subcommands:").unwrap();
        for subcommand in subcommands {
            let name = subcommand.get_name();
            let about = subcommand.get_about();
            write!(markdown, "* `{name}`").unwrap();
            if let Some(about) = about {
                write!(markdown, ": {about}").unwrap();
            }
            writeln!(markdown).unwrap();
        }
        writeln!(markdown).unwrap();
    }

    // arguments
    let pos = cmd.get_positionals().collect::<Vec<_>>();
    if !pos.is_empty() {
        writeln!(markdown, "### Arguments:").unwrap();
        for arg in pos {
            write_arg_markdown(&mut markdown, arg).unwrap();
        }
        writeln!(markdown).unwrap();
    }

    // common options link
    writeln!(
        markdown,
        "### Common Options:\n\n\
        Common options are documented on the [Common Options](../common-options) page.\n"
    )
    .unwrap();

    // options
    let options = cmd
        .get_arguments()
        .filter(|arg| {
            if arg.is_positional() || arg.is_hide_set() {
                return false;
            }
            // Filter out common options
            if let Some(help_heading) = arg.get_help_heading() {
                !COMMON_OPTION_HEADINGS.contains(&help_heading)
            } else {
                true
            }
        })
        .collect::<Vec<_>>();

    if !options.is_empty() {
        writeln!(markdown, "### Options:").unwrap();
        for arg in options {
            write_arg_markdown(&mut markdown, arg).unwrap();
        }
        writeln!(markdown).unwrap();
    }

    let name = cmd.get_name();
    path.push(name.to_owned());

    // recursively generate subcommand docs
    for subcommand in cmd.get_subcommands() {
        let sub_content = cmd_content_markdown(subcommand, path.clone())?;
        markdown.push_str(&sub_content);
    }

    markdown.push_str("\n\n");

    Ok(markdown)
}

fn write_arg_markdown(buffer: &mut String, arg: &clap::Arg) -> std::fmt::Result {
    //--------------------
    // Cmd arg or option
    // e.g. * `--opt`
    //--------------------
    write!(buffer, "* ")?;

    let value_name = arg
        .get_value_names()
        .map(|val| val.join(" "))
        .unwrap_or(arg.get_id().to_string().to_ascii_uppercase());

    let opt_prefix = match (arg.get_short(), arg.get_long()) {
        (Some(short), Some(long)) => {
            format!("-{short}, --{long}")
        }
        (Some(short), None) => {
            format!("-{short}")
        }
        (None, Some(long)) => {
            format!("--{long}")
        }
        (None, None) => "".to_owned(),
    };

    if opt_prefix.is_empty() {
        write!(buffer, "`<{value_name}>`")?;
    } else if arg.get_action().takes_values() {
        write!(buffer, "`{opt_prefix} <{value_name}>`")?;
    } else {
        write!(buffer, "`{opt_prefix}`")?;
    }

    //--------------------
    // Cmd flag aliases
    //--------------------

    if let Some(aliases) = arg.get_visible_aliases().as_deref() {
        if !aliases.is_empty() {
            let aliases_str = aliases
                .iter()
                .map(|a| format!("`--{a}`"))
                .collect::<Vec<String>>()
                .join(", ");
            write!(buffer, " (alias: {aliases_str})")?;
        }
    }

    writeln!(buffer)?;

    let indent = "    ";

    //--------------------
    // Help message
    //--------------------

    let help = arg.get_long_help().or(arg.get_help());
    if let Some(help) = help {
        // for each line of help, indent it by 4 spaces
        let content = help
            .to_string()
            .lines()
            .map(|line| format!("{indent}{line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let content = escape_angle_brackets_for_mdx(&content);
        writeln!(buffer, "{content}")?;
    }

    writeln!(buffer)?;

    //--------------------
    // Arg default values
    //--------------------

    if !arg.get_default_values().is_empty() {
        let default_values: String = arg
            .get_default_values()
            .iter()
            .map(|value| format!("`{}`", value.to_string_lossy()))
            .collect::<Vec<String>>()
            .join(", ");

        if arg.get_default_values().len() > 1 {
            // Plural
            writeln!(buffer, "{indent}* Default values: {default_values}")?;
        } else {
            // Singular
            writeln!(buffer, "{indent}* Default value: {default_values}")?;
        }
    }

    //--------------------
    // Arg possible values
    //--------------------

    let possible_values: Vec<PossibleValue> = arg
        .get_possible_values()
        .into_iter()
        .filter(|pv| !pv.is_hide_set())
        .collect();

    // Print possible values for options that take a value, but not for flags
    // that can only be either present or absent and do not take a value.
    if !possible_values.is_empty() && !matches!(arg.get_action(), clap::ArgAction::SetTrue) {
        let possible_values_content = possible_values
            .iter()
            .map(|pv| {
                let name = pv.get_name();
                if let Some(help) = pv.get_help() {
                    format!("{indent}{indent}* `{name}`: {help}")
                } else {
                    format!("{indent}{indent}* `{name}`")
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        writeln!(
            buffer,
            "{indent}* Possible values:\n{possible_values_content}"
        )?;
    }

    Ok(())
}

/// Escapes angle brackets in the given string to avoid breaking the MDX parser.
///
/// Specifically, it replaces all `<` with `&lt;` and all `>` with `&gt;`
/// except when they appear inside inline or block code spans.
fn escape_angle_brackets_for_mdx(input: &str) -> String {
    // Build a regex that matches either:
    // 1. A fenced code block: ```...``` or a backtick-enclosed inline code: `...`
    //    (the fence must come first: the inline alternative also matches the start of a fence)
    // 2. A single `<` or `>` character
    let re = Regex::new(r"(?s)(```.*?```|`[^`]*`)|(<|>)").unwrap();

    // Perform the replacement for all captured groups
    re.replace_all(input, |caps: &regex::Captures| {
        if caps.get(2).is_some() {
            if &caps[2] == "<" {
                "&lt;".to_owned()
            } else {
                "&gt;".to_owned()
            }
        } else {
            // Otherwise, it was a code span; leave it unchanged
            caps[0].to_string()
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use clap::Arg;
    use clap::Command;

    use super::cmd_markdown_help;
    use super::escape_angle_brackets_for_mdx;

    #[test]
    fn test_simple_escape() {
        let input = "This <string> should be escaped.";
        let expected = "This &lt;string&gt; should be escaped.";
        assert_eq!(escape_angle_brackets_for_mdx(input), expected);
    }

    #[test]
    fn test_no_escape_in_single_backticks() {
        let input = "Do not escape `<p>code</p>` here.";
        let expected = "Do not escape `<p>code</p>` here.";
        assert_eq!(escape_angle_brackets_for_mdx(input), expected);
    }

    #[test]
    fn test_no_escape_in_triple_backticks() {
        let input = "```\nlet x = 5 < 10;\n```";
        let expected = "```\nlet x = 5 < 10;\n```";
        assert_eq!(escape_angle_brackets_for_mdx(input), expected);
    }

    #[test]
    fn test_multiline_triple_backticks() {
        let input = r#"
```html
<div>
  <p>Hello World</p>
</div>
```
"#;
        let expected = r#"
```html
<div>
  <p>Hello World</p>
</div>
```
"#;
        assert_eq!(escape_angle_brackets_for_mdx(input), expected);
    }

    #[test]
    fn test_mixed_content() {
        let input = "Escape <this>, but not `this <one>` or ```\nthis <one> either\n```. But escape <this> again.";
        let expected = "Escape &lt;this&gt;, but not `this <one>` or ```\nthis <one> either\n```. But escape &lt;this&gt; again.";
        assert_eq!(escape_angle_brackets_for_mdx(input), expected);
    }

    #[test]
    fn test_adjacent_to_code_blocks() {
        let input = "<tag>`code<code>`</tag>";
        let expected = "&lt;tag&gt;`code<code>`&lt;/tag&gt;";
        assert_eq!(escape_angle_brackets_for_mdx(input), expected);
    }

    /// Braces in the about text reach the page unchanged.
    #[test]
    fn uquery_page_keeps_select_encoding_example() {
        const ABOUT: &str = "`1 + select({\"//:a\": 1, \"DEFAULT\": 2})` will be encoded as:\n\n\
            `{\"__type\": \"concat\", \"items\": [1, {\"__type\": \"selector\", \
            \"entries\": {\"//:a\": 1, \"DEFAULT\": 2}}]}`\n";
        // ast-grep-ignore: rust/buck2-no-command-new
        let md = cmd_markdown_help(&Command::new("uquery").long_about(ABOUT));
        assert!(md.contains(ABOUT), "{md}");
        assert!(md.contains("**Usage**: `buck2 uquery`"), "{md}");
    }

    /// Text that looks like a template tag inside an option's help is printed literally.
    #[test]
    fn template_tags_in_option_help_are_literal() {
        // ast-grep-ignore: rust/buck2-no-command-new
        let cmd = Command::new("tagdemo").arg(
            Arg::new("fmt")
                .long("fmt")
                .help("Placeholders: {name} is replaced by the target name"),
        );
        let md = cmd_markdown_help(&cmd);
        assert!(
            md.contains("Placeholders: {name} is replaced by the target name"),
            "{md}"
        );
    }

    /// A fenced block is left alone whatever backticks it contains.
    #[test]
    fn angle_brackets_inside_fence_after_odd_backtick_are_kept() {
        let input = "```\necho `date\ncat <file>\n```";
        assert_eq!(escape_angle_brackets_for_mdx(input), input);
    }
}
