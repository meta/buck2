/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use buck2_interpreter::types::cell_root::CellRoot;
use buck2_interpreter::types::project_root::StarlarkProjectRoot;
use buck2_util::size_assert;
use derive_more::Display;
use display_container::fmt_container;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use mini_vec::MiniVec;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::string::StarlarkStr;
use starlark::values::type_repr::StarlarkTypeRepr;
use strong_hash::StrongHash;

use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifactView;
use crate::interpreter::rule_defs::cmd_args::regex::CmdArgsRegex;

/// Supported ways of quoting arguments.
#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    Trace,
    Freeze,
    Serialize,
    Allocative,
    Pagable,
    StarlarkPagable,
    StrongHash
)]
pub enum QuoteStyle {
    /// Quote arguments for Unix shell:
    /// <https://pubs.opengroup.org/onlinepubs/9699919799/utilities/V3_chap02.html>
    Shell,
}

impl Display for QuoteStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shell => write!(f, "shell"),
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum CommandLineArgError {
    #[error("Unknown quoting style `{0}`")]
    UnknownQuotingStyle(String),
}

impl QuoteStyle {
    pub fn parse(s: &str) -> buck2_error::Result<QuoteStyle> {
        match s {
            "shell" => Ok(QuoteStyle::Shell),
            _ => Err(CommandLineArgError::UnknownQuotingStyle(s.to_owned()).into()),
        }
    }
}

pub(crate) trait CommandLineOptionsTrait<'v> {
    fn ignore_artifacts(&self) -> bool;
    fn salt_hidden(&self) -> bool;
    fn delimiter(&self) -> Option<StringValue<'v>>;

    fn to_command_line_options<'a>(&'a self) -> CommandLineOptionsRef<'v, 'a>;
}

#[derive(Debug, Default, Clone, Trace, Allocative)]
#[repr(C)]
pub(crate) struct CommandLineOptions<'v> {
    // These impact how artifacts are rendered
    pub(crate) relative_to: Option<(ValueOf<'v, RelativeOrigin<'v>>, u32)>,
    pub(crate) absolute_prefix: Option<StringValue<'v>>,
    pub(crate) absolute_suffix: Option<StringValue<'v>>,
    pub(crate) parent: u32,
    pub(crate) ignore_artifacts: bool,
    pub(crate) salt_hidden: bool,

    // These impact the formatting of each string
    pub(crate) delimiter: Option<StringValue<'v>>,
    pub(crate) format: Option<StringValue<'v>>,
    pub(crate) prepend: Option<StringValue<'v>>,
    pub(crate) quote: Option<QuoteStyle>,
    pub(crate) replacements: MiniVec<(CmdArgsRegex<'v>, StringValue<'v>)>,
}

#[derive(Clone, Copy, Dupe, Default)]
pub(crate) struct OptionsReplacementsRef<'v, 'a>(&'a [(CmdArgsRegex<'v>, StringValue<'v>)]);

impl<'v, 'a> Display for OptionsReplacementsRef<'v, 'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let iter = self.iter();
        fmt_container(
            f,
            "[",
            "]",
            iter.map(|(r, s)| {
                struct D<'v>(CmdArgsRegex<'v>, StringValue<'v>);
                impl<'v> Display for D<'v> {
                    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                        write!(f, "({}, {})", StarlarkStr::repr(self.0.as_str()), self.1)
                    }
                }
                D(r, s)
            }),
        )
    }
}

impl<'v, 'a> OptionsReplacementsRef<'v, 'a> {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (CmdArgsRegex<'v>, StringValue<'v>)> + use<'v, 'a> {
        self.0.iter().copied()
    }
}

impl<'v, 'a> Serialize for OptionsReplacementsRef<'v, 'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[derive(Default, Serialize)]
pub(crate) struct CommandLineOptionsRef<'v, 'a> {
    #[serde(serialize_with = "serialize_opt_display")]
    pub(crate) relative_to: Option<(ValueOf<'v, RelativeOrigin<'v>>, u32)>,
    pub(crate) absolute_prefix: Option<StringValue<'v>>,
    pub(crate) absolute_suffix: Option<StringValue<'v>>,
    pub(crate) parent: u32,
    pub(crate) ignore_artifacts: bool,
    pub(crate) salt_hidden: bool,

    pub(crate) delimiter: Option<StringValue<'v>>,
    pub(crate) format: Option<StringValue<'v>>,
    pub(crate) prepend: Option<StringValue<'v>>,
    pub(crate) quote: Option<QuoteStyle>,
    pub(crate) replacements: OptionsReplacementsRef<'v, 'a>,
}

impl<'v, 'a> CommandLineOptionsRef<'v, 'a> {
    pub(crate) fn to_owned(&self) -> CommandLineOptions<'v> {
        CommandLineOptions {
            relative_to: self.relative_to,
            absolute_prefix: self.absolute_prefix,
            absolute_suffix: self.absolute_suffix,
            parent: self.parent,
            ignore_artifacts: self.ignore_artifacts,
            salt_hidden: self.salt_hidden,
            delimiter: self.delimiter,
            format: self.format,
            prepend: self.prepend,
            quote: self.quote.dupe(),
            replacements: self.replacements.iter().collect(),
        }
    }
}

impl<'v> CommandLineOptionsTrait<'v> for CommandLineOptions<'v> {
    fn ignore_artifacts(&self) -> bool {
        self.ignore_artifacts
    }

    fn salt_hidden(&self) -> bool {
        self.salt_hidden
    }

    fn delimiter(&self) -> Option<StringValue<'v>> {
        self.delimiter.dupe()
    }

    fn to_command_line_options<'a>(&'a self) -> CommandLineOptionsRef<'v, 'a> {
        CommandLineOptionsRef {
            relative_to: self.relative_to,
            absolute_prefix: self.absolute_prefix,
            absolute_suffix: self.absolute_suffix,
            parent: self.parent,
            ignore_artifacts: self.ignore_artifacts,
            salt_hidden: self.salt_hidden,
            delimiter: self.delimiter,
            format: self.format,
            prepend: self.prepend,
            quote: self.quote.dupe(),
            replacements: OptionsReplacementsRef(self.replacements.as_slice()),
        }
    }
}

#[derive(Debug, Allocative, StarlarkPagable)]
enum FrozenCommandLineOption<'v> {
    RelativeTo(ValueOf<'v, RelativeOrigin<'v>>, u32),
    AbsolutePrefix(StringValue<'v>),
    AbsoluteSuffix(StringValue<'v>),
    Parent(u32),
    IgnoreArtifacts,
    SaltHidden,
    Delimiter(StringValue<'v>),
    Format(StringValue<'v>),
    Prepend(StringValue<'v>),
    Quote(#[starlark_pagable(pagable)] QuoteStyle),
    Replacements(MiniBoxSlice<(CmdArgsRegex<'v>, StringValue<'v>)>),
}

size_assert::words_of_type!(FrozenCommandLineOption<'static>, 2);

#[derive(Debug, Default, ProvidesStaticType, Allocative, StarlarkPagable)]
pub(crate) struct FrozenCommandLineOptions<'v> {
    options: MiniBoxSlice<FrozenCommandLineOption<'v>>,
}

impl<'v> FrozenCommandLineOptions<'v> {
    pub const fn empty() -> Self {
        FrozenCommandLineOptions {
            options: MiniBoxSlice::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.options.is_empty()
    }
}

impl<'v> CommandLineOptionsTrait<'v> for FrozenCommandLineOptions<'v> {
    fn ignore_artifacts(&self) -> bool {
        for option in self.options.iter() {
            if let FrozenCommandLineOption::IgnoreArtifacts = option {
                return true;
            }
        }
        false
    }

    fn salt_hidden(&self) -> bool {
        for option in self.options.iter() {
            if let FrozenCommandLineOption::SaltHidden = option {
                return true;
            }
        }
        false
    }

    fn delimiter(&self) -> Option<StringValue<'v>> {
        for option in self.options.iter() {
            if let FrozenCommandLineOption::Delimiter(value) = option {
                return Some(*value);
            }
        }
        None
    }

    fn to_command_line_options<'a>(&'a self) -> CommandLineOptionsRef<'v, 'a> {
        let mut options = CommandLineOptionsRef::default();
        for option in &*self.options {
            match option {
                FrozenCommandLineOption::RelativeTo(value, parent) => {
                    options.relative_to = Some((*value, *parent));
                }
                FrozenCommandLineOption::AbsolutePrefix(value) => {
                    options.absolute_prefix = Some(*value);
                }
                FrozenCommandLineOption::AbsoluteSuffix(value) => {
                    options.absolute_suffix = Some(*value);
                }
                FrozenCommandLineOption::Parent(parent) => {
                    options.parent = *parent;
                }
                FrozenCommandLineOption::IgnoreArtifacts => {
                    options.ignore_artifacts = true;
                }
                FrozenCommandLineOption::SaltHidden => {
                    options.salt_hidden = true;
                }
                FrozenCommandLineOption::Delimiter(value) => {
                    options.delimiter = Some(*value);
                }
                FrozenCommandLineOption::Format(value) => {
                    options.format = Some(*value);
                }
                FrozenCommandLineOption::Prepend(value) => {
                    options.prepend = Some(*value);
                }
                FrozenCommandLineOption::Quote(value) => {
                    options.quote = Some(value.dupe());
                }
                FrozenCommandLineOption::Replacements(value) => {
                    options.replacements = OptionsReplacementsRef(value);
                }
            }
        }
        options
    }
}

impl<'v> Serialize for CommandLineOptions<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_command_line_options().serialize(serializer)
    }
}

impl<'v> Serialize for FrozenCommandLineOptions<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_command_line_options().serialize(serializer)
    }
}

impl<'v> Freeze<'v> for CommandLineOptions<'v> {
    type Frozen<'fv> = FrozenCommandLineOptions<'fv>;

    fn freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<FrozenCommandLineOptions<'fv>> {
        let CommandLineOptions {
            relative_to,
            absolute_prefix,
            absolute_suffix,
            parent,
            ignore_artifacts,
            salt_hidden,
            delimiter,
            format,
            prepend,
            quote,
            replacements,
        } = self;

        let mut options = Vec::new();
        if let Some((relative_to, parent)) = relative_to {
            options.push(FrozenCommandLineOption::RelativeTo(
                Freeze::freeze(relative_to, freezer)?,
                parent,
            ));
        }
        if let Some(absolute_prefix) = absolute_prefix {
            let absolute_prefix = Freeze::freeze(absolute_prefix, freezer)?;
            options.push(FrozenCommandLineOption::AbsolutePrefix(absolute_prefix));
        }
        if let Some(absolute_suffix) = absolute_suffix {
            let absolute_suffix = Freeze::freeze(absolute_suffix, freezer)?;
            options.push(FrozenCommandLineOption::AbsoluteSuffix(absolute_suffix));
        }
        if parent != 0 {
            options.push(FrozenCommandLineOption::Parent(parent));
        }
        if ignore_artifacts {
            options.push(FrozenCommandLineOption::IgnoreArtifacts);
        }
        if salt_hidden {
            options.push(FrozenCommandLineOption::SaltHidden);
        }
        if let Some(delimiter) = delimiter {
            let delimiter = Freeze::freeze(delimiter, freezer)?;
            options.push(FrozenCommandLineOption::Delimiter(delimiter));
        }
        if let Some(format) = format {
            let format = Freeze::freeze(format, freezer)?;
            options.push(FrozenCommandLineOption::Format(format));
        }
        if let Some(prepend) = prepend {
            let prepend = Freeze::freeze(prepend, freezer)?;
            options.push(FrozenCommandLineOption::Prepend(prepend));
        }
        if let Some(quote) = quote {
            options.push(FrozenCommandLineOption::Quote(quote));
        }
        if !replacements.is_empty() {
            let replacements = MiniBoxSlice::from(Freeze::freeze(replacements, freezer)?);
            options.push(FrozenCommandLineOption::Replacements(replacements));
        }

        Ok(FrozenCommandLineOptions {
            options: MiniBoxSlice::from_iter(options),
        })
    }
}

fn serialize_opt_display<V: Display, S>(v: &Option<(V, u32)>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match v {
        Some((v, u)) => s.serialize_some(&(format!("{v}"), u)),
        None => s.serialize_none(),
    }
}

// NOTE: This is an enum as opposed to a trait because of the `C` parameter on (which is required
// because upcasting is not stable).
#[derive(Display, StarlarkTypeRepr, UnpackValue, ProvidesStaticType)]
pub(crate) enum RelativeOrigin<'v> {
    OutputArtifact(StarlarkOutputArtifactView<'v>),
    Artifact(StarlarkArtifactView<'v>),
    CellRoot(&'v CellRoot),
    /// Bit of a useless variant since this is simply the default, but we allow it for consistency.
    ProjectRoot(&'v StarlarkProjectRoot),
}

impl<'v, 'x> CommandLineOptionsRef<'v, 'x> {
    pub(crate) fn changes_builder(&self) -> bool {
        match self {
            Self {
                relative_to: None,
                absolute_prefix: None,
                absolute_suffix: None,
                parent: 0,
                delimiter: None,
                format: None,
                prepend: None,
                quote: None,
                replacements,
                ignore_artifacts: _, // Doesn't impact the builder
                salt_hidden: _,      // Doesn't impact the builder
            } if replacements.is_empty() => false,
            _ => true,
        }
    }

    pub(crate) fn iter_fields_display(
        &self,
    ) -> impl Iterator<Item = (&'static str, CommandLineOptionsIterItem<'v, 'x>)> + use<'v, 'x>
    {
        let CommandLineOptionsRef {
            relative_to,
            absolute_prefix,
            absolute_suffix,
            parent,
            ignore_artifacts,
            salt_hidden,
            delimiter,
            format,
            prepend,
            quote,
            replacements,
        } = self;

        // This can be implemented without allocation,
        // but generic version with iterator chain chain chain...
        // or either either either... leads to compilation of the crate slowing down
        // from 15s to minutes, and a fight against the borrow checker.
        let mut iter = Vec::new();

        if let Some((value, index)) = relative_to {
            iter.push((
                "relative_to",
                CommandLineOptionsIterItem::Value(value.to_value()),
            ));
            if *index != 0 {
                iter.push((
                    "relative_to_parent",
                    CommandLineOptionsIterItem::U32(*index),
                ));
            }
        }

        if let Some(value) = absolute_prefix {
            iter.push((
                "absolute_prefix",
                CommandLineOptionsIterItem::StringValue(*value),
            ));
        }
        if let Some(value) = absolute_suffix {
            iter.push((
                "absolute_suffix",
                CommandLineOptionsIterItem::StringValue(*value),
            ));
        }
        if *parent != 0 {
            iter.push(("parent", CommandLineOptionsIterItem::U32(*parent)));
        }
        if *ignore_artifacts {
            iter.push(("ignore_artifacts", CommandLineOptionsIterItem::Str("True")));
        }
        if *salt_hidden {
            iter.push((
                "experimental_salt_hidden",
                CommandLineOptionsIterItem::Str("True"),
            ));
        }
        if let Some(value) = delimiter {
            iter.push(("delimiter", CommandLineOptionsIterItem::StringValue(*value)));
        }
        if let Some(value) = format {
            iter.push(("format", CommandLineOptionsIterItem::StringValue(*value)));
        }
        if let Some(value) = prepend {
            iter.push(("prepend", CommandLineOptionsIterItem::StringValue(*value)));
        }
        if let Some(value) = quote {
            iter.push(("quote", CommandLineOptionsIterItem::QuoteStyle(*value)));
        }
        if !replacements.is_empty() {
            iter.push((
                "replacements",
                CommandLineOptionsIterItem::Replacements(*replacements),
            ));
        }

        iter.into_iter()
    }
}

#[derive(derive_more::Display)]
pub(crate) enum CommandLineOptionsIterItem<'v, 'a> {
    U32(u32),
    Value(Value<'v>),
    Str(&'static str),
    StringValue(StringValue<'v>),
    Replacements(OptionsReplacementsRef<'v, 'a>),
    #[display("\"{}\"", _0)]
    QuoteStyle(QuoteStyle),
}
