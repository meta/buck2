/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Ref;
use std::cell::RefCell;
use std::cell::RefMut;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutInsertionSet;
use buck2_util::size_assert;
use display_container::display_pair;
use display_container::fmt_container;
use display_container::iter_display_chain;
use dupe::Dupe;
use either::Either;
use mini_vec::MiniVec;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Arguments;
use starlark::static_starlark_value;
use starlark::typing::Ty;
use starlark::values::AllocValue;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::StaticFreezePlan;
use starlark::values::StringValue;
use starlark::values::ThinBoxSliceValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListRef;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::tuple::TupleType;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use crate::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use crate::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::builder::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;
use crate::interpreter::rule_defs::cmd_args::compute_relative_to_path;
use crate::interpreter::rule_defs::cmd_args::options::CommandLineOptions;
use crate::interpreter::rule_defs::cmd_args::options::CommandLineOptionsRef;
use crate::interpreter::rule_defs::cmd_args::options::CommandLineOptionsTrait;
use crate::interpreter::rule_defs::cmd_args::options::FrozenCommandLineOptions;
use crate::interpreter::rule_defs::cmd_args::options::QuoteStyle;
use crate::interpreter::rule_defs::cmd_args::options::RelativeOrigin;
use crate::interpreter::rule_defs::cmd_args::regex::CmdArgsRegex;
use crate::interpreter::rule_defs::cmd_args::traits::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::traits::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::traits::InputProperties;
use crate::interpreter::rule_defs::cmd_args::traits::Requires;
use crate::interpreter::rule_defs::cmd_args::traits::SimpleCommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::traits::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::value::CommandLineArg;

#[derive(Debug, buck2_error::Error)]
pub enum CommandLineError {
    #[error(
        "Artifact(s) {0:?} are content-based and `hidden` under `ignore_artifacts`: nothing renders them, so nothing would carry their content to the action"
    )]
    #[buck2(input)]
    HiddenContentBasedIgnoreArtifacts(BuckMutInsertionSet<String>),
}

/// Forwards to `inner`, rewriting the properties of every input below.
struct MapProperties<'a, 'v, F> {
    inner: &'a mut dyn CommandLineArtifactVisitor<'v>,
    map: F,
}

/// Entries of a `cmd_args`'s `hidden` list: every input below is reported as hidden.
fn hidden_visitor<'a, 'v>(
    inner: &'a mut dyn CommandLineArtifactVisitor<'v>,
) -> MapProperties<'a, 'v, impl Fn(InputProperties) -> InputProperties> {
    MapProperties {
        inner,
        map: |props| InputProperties {
            hidden: true,
            ..props
        },
    }
}

impl<'v, F: Fn(InputProperties) -> InputProperties> CommandLineArtifactVisitor<'v>
    for MapProperties<'_, 'v, F>
{
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        props: InputProperties,
        tags: Vec<&ArtifactTag>,
    ) {
        self.inner.visit_input(input, (self.map)(props), tags);
    }

    fn visit_declared_artifact(
        &mut self,
        declared_artifact: buck2_artifact::artifact::artifact_type::DeclaredArtifact<'v>,
        props: InputProperties,
        tags: Vec<&ArtifactTag>,
    ) -> buck2_error::Result<()> {
        self.inner
            .visit_declared_artifact(declared_artifact, (self.map)(props), tags)
    }

    fn visit_declared_output(&mut self, artifact: OutputArtifact<'v>, tags: Vec<&ArtifactTag>) {
        self.inner.visit_declared_output(artifact, tags);
    }

    fn visit_frozen_output(&mut self, artifact: Artifact, tags: Vec<&ArtifactTag>) {
        self.inner.visit_frozen_output(artifact, tags);
    }

    fn push_frame(&mut self) -> buck2_error::Result<()> {
        self.inner.push_frame()
    }

    fn pop_frame(&mut self) {
        self.inner.pop_frame()
    }

    fn skip_hidden(&self) -> bool {
        self.inner.skip_hidden()
    }
}

/// Fields of `cmd_args`. Abstract mutable and frozen versions.
trait Fields<'v> {
    fn items(&self) -> &[CommandLineArg<'v>];
    fn hidden(&self) -> &[CommandLineArg<'v>];
    fn options(&self) -> Option<&dyn CommandLineOptionsTrait<'v>>;
}

/// Wrapper because we cannot implement traits for traits.
struct FieldsRef<'v, F: Fields<'v>>(F, PhantomData<Value<'v>>);

/// There's no good reason for a user to write `cmd_args` as JSON in analysis or BXL.
///
/// This implementation exists for operations such as:
///
/// ```ignore
/// buck2 cquery :buck2 --providers
/// ```
///
/// which must not fail if a provider contains `cmd_args` (D34887765).
impl<'v, F: Fields<'v>> Serialize for FieldsRef<'v, F> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        /// Make sure mutable and frozen `cmd_args` are serialized identically
        /// by routing through this struct.
        #[derive(Serialize)]
        struct Mirror<'v, 'a> {
            items: &'a [CommandLineArg<'v>],
            hidden: &'a [CommandLineArg<'v>],
            options: Option<CommandLineOptionsRef<'v, 'a>>,
        }

        Mirror {
            items: self.0.items(),
            hidden: self.0.hidden(),
            options: self.0.options().map(|x| x.to_command_line_options()),
        }
        .serialize(serializer)
    }
}

impl<'v, F: Fields<'v>> Display for FieldsRef<'v, F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_container(
            f,
            "cmd_args(",
            ")",
            iter_display_chain(
                self.0.items(),
                iter_display_chain(
                    Some(self.0.hidden())
                        .filter(|x| !x.is_empty())
                        .map(|hidden| {
                            struct Wrapper<'a, 'v>(&'a [CommandLineArg<'v>]);
                            impl<'a, 'v> Display for Wrapper<'a, 'v> {
                                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                                    fmt_container(f, "[", "]", self.0.iter())
                                }
                            }
                            display_pair("hidden", "=", Wrapper(hidden))
                        }),
                    self.0
                        .options()
                        .map(|o| o.to_command_line_options())
                        .unwrap_or_default()
                        .iter_fields_display()
                        .map(|(k, v)| display_pair(k, "=", v)),
                ),
            ),
        )
    }
}

impl<'v, F: Fields<'v>> FieldsRef<'v, F> {
    fn copy(&self) -> StarlarkCmdArgs<'v> {
        StarlarkCmdArgs(RefCell::new(StarlarkCommandLineData {
            items: self.0.items().into(),
            hidden: self.0.hidden().into(),
            options: self
                .0
                .options()
                .map(|x| Box::new(x.to_command_line_options().to_owned())),
        }))
    }

    fn ignore_artifacts(&self) -> bool {
        self.0
            .options()
            .map(|o| o.ignore_artifacts())
            .unwrap_or_default()
    }

    fn salt_hidden(&self) -> bool {
        self.0
            .options()
            .map(|o| o.salt_hidden())
            .unwrap_or_default()
    }

    fn is_concat(&self) -> bool {
        if let Some(x) = &self.0.options() {
            x.delimiter().is_some()
        } else {
            false
        }
    }
}

impl<'v, F: Fields<'v>> CommandLineArgLike<'v> for FieldsRef<'v, F> {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkCmdArgs::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        match self.0.options() {
            None => {
                for item in self.0.items() {
                    item.as_command_line_arg().add_to_command_line(fmt)?;
                }
                Ok(())
            }
            Some(options) => {
                let options = options.to_command_line_options();
                if options.changes_builder() {
                    fmt.push_scope(&options)?;
                }
                for item in self.0.items() {
                    item.as_command_line_arg().add_to_command_line(fmt)?;
                }
                if options.changes_builder() {
                    fmt.pop_scope();
                }
                Ok(())
            }
        }
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        fn visit_items<'a>(
            visitor: &mut dyn CommandLineArtifactVisitor<'a>,
            items: &[CommandLineArg<'a>],
        ) -> buck2_error::Result<()> {
            for item in items {
                visitor.push_frame()?;
                item.as_command_line_arg().visit_artifacts(visitor)?;
                visitor.pop_frame();
            }

            Ok(())
        }

        // The salted hidden list asks for digests, whatever else the scope says.
        fn visit_hidden<'a>(
            visitor: &mut dyn CommandLineArtifactVisitor<'a>,
            hidden: &[CommandLineArg<'a>],
            salt: bool,
        ) -> buck2_error::Result<()> {
            let mut hidden_visitor = MapProperties {
                inner: visitor,
                map: move |props: InputProperties| {
                    let InputProperties {
                        hidden: _,
                        requires,
                    } = props;
                    InputProperties {
                        hidden: true,
                        requires: if salt { Requires::Digest } else { requires },
                    }
                },
            };
            visit_items(&mut hidden_visitor, hidden)
        }

        let salt_hidden = self.salt_hidden();
        if !self.ignore_artifacts() {
            visit_items(visitor, self.0.items())?;
            if !visitor.skip_hidden() {
                visit_hidden(visitor, self.0.hidden(), salt_hidden)?;
            }
            return Ok(());
        }

        /// Forwards, as `Requires::Path`, the artifacts whose paths require them to be
        /// built; drops the rest. Rejects a content-based artifact hidden in this scope,
        /// since nothing would render it.
        struct PathOnlyVisitor<'a, 'v> {
            inner: &'a mut dyn CommandLineArtifactVisitor<'v>,
            rejected: BuckMutInsertionSet<String>,
        }

        impl<'v> CommandLineArtifactVisitor<'v> for PathOnlyVisitor<'_, 'v> {
            fn visit_input(
                &mut self,
                input: ArtifactGroup,
                props: InputProperties,
                tags: Vec<&ArtifactTag>,
            ) {
                let InputProperties {
                    hidden,
                    requires: _,
                } = props;
                if !input.path_resolution_may_require_artifact_value() {
                    return;
                }
                if hidden {
                    self.rejected.insert(input.to_string());
                    return;
                }
                self.inner.visit_input(
                    input,
                    InputProperties {
                        requires: Requires::Path,
                        ..props
                    },
                    tags,
                );
            }

            fn visit_declared_artifact(
                &mut self,
                declared_artifact: buck2_artifact::artifact::artifact_type::DeclaredArtifact<'v>,
                props: InputProperties,
                tags: Vec<&ArtifactTag>,
            ) -> buck2_error::Result<()> {
                let InputProperties {
                    hidden,
                    requires: _,
                } = props;
                if !declared_artifact.has_content_based_path() {
                    return Ok(());
                }
                if hidden {
                    self.rejected.insert(declared_artifact.to_string());
                    return Ok(());
                }
                self.inner.visit_input(
                    ArtifactGroup::Artifact(declared_artifact.ensure_bound()?.into_artifact()),
                    InputProperties {
                        requires: Requires::Path,
                        ..props
                    },
                    tags,
                );
                Ok(())
            }

            fn visit_declared_output(
                &mut self,
                _artifact: OutputArtifact<'v>,
                _tags: Vec<&ArtifactTag>,
            ) {
            }

            fn visit_frozen_output(&mut self, _artifact: Artifact, _tags: Vec<&ArtifactTag>) {}

            fn push_frame(&mut self) -> buck2_error::Result<()> {
                self.inner.push_frame()
            }

            fn pop_frame(&mut self) {
                self.inner.pop_frame()
            }

            fn skip_hidden(&self) -> bool {
                self.inner.skip_hidden()
            }
        }

        let mut path_only_visitor = PathOnlyVisitor {
            inner: visitor,
            rejected: BuckMutInsertionSet::default(),
        };
        visit_items(&mut path_only_visitor, self.0.items())?;
        let skip_hidden = path_only_visitor.skip_hidden();
        if !skip_hidden && !salt_hidden {
            visit_items(&mut hidden_visitor(&mut path_only_visitor), self.0.hidden())?;
        }
        let PathOnlyVisitor {
            inner: visitor,
            rejected,
        } = path_only_visitor;
        if !rejected.is_empty() {
            return Err(CommandLineError::HiddenContentBasedIgnoreArtifacts(rejected).into());
        }
        if !skip_hidden && salt_hidden {
            visit_hidden(visitor, self.0.hidden(), true)?;
        }
        Ok(())
    }

    fn contains_arg_attr(&self) -> bool {
        self.0
            .items()
            .iter()
            .any(|x| x.as_command_line_arg().contains_arg_attr())
            || self
                .0
                .hidden()
                .iter()
                .any(|x| x.as_command_line_arg().contains_arg_attr())
    }

    fn visit_write_to_file_macros(
        &self,
        visitor: &mut dyn WriteToFileMacroVisitor,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        if let Some(options) = &self.0.options() {
            if let Some((p, parents)) = options.to_command_line_options().relative_to {
                if let Some(fs) = visitor.fs() {
                    visitor.set_current_relative_to_path(compute_relative_to_path(
                        p,
                        parents,
                        fs.fs(),
                        artifact_path_mapping,
                    )?);
                }
            }
        }

        for item in self.0.items() {
            item.as_command_line_arg()
                .visit_write_to_file_macros(visitor, artifact_path_mapping)?;
        }
        for item in self.0.hidden() {
            item.as_command_line_arg()
                .visit_write_to_file_macros(visitor, artifact_path_mapping)?;
        }
        Ok(())
    }
}

/// Starlark object returned by `cmd_args()`
/// A container for all of the args and nested command lines that a users adds to `ctx.args()`
///
/// This allows more efficient iterative argument building, including O(1) insertion of
/// `CommandLine` / `FrozenCommandLine` args.
///
/// When frozen, a `FrozenCommandLine` is created.
///
/// `items` contains strings, artifacts, command line args (frozen and not), but does not
///         contain any builders.
#[derive(Debug, Default, Clone, Trace, ProvidesStaticType, Allocative)]
#[repr(C)]
pub struct StarlarkCommandLineData<'v> {
    items: MiniVec<CommandLineArg<'v>>,
    hidden: MiniVec<CommandLineArg<'v>>,
    options: Option<Box<CommandLineOptions<'v>>>,
}

#[derive(Debug, Default, Clone, Trace, ProvidesStaticType, Allocative)]
pub struct StarlarkCmdArgs<'v>(RefCell<StarlarkCommandLineData<'v>>);

impl<'v> Serialize for StarlarkCmdArgs<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        FieldsRef(self.0.borrow(), PhantomData).serialize(serializer)
    }
}

#[derive(Debug, ProvidesStaticType, Allocative, StarlarkPagable)]
pub struct FrozenStarlarkCmdArgs<'v> {
    // Elements are frozen `CommandLineArg`s
    items: ThinBoxSliceValue<'v>,
    hidden: ThinBoxSliceValue<'v>,
    options: FrozenCommandLineOptions<'v>,
}

static_starlark_value!(EMPTY_FROZEN_CMD_ARGS: FrozenStarlarkCmdArgs<'static> = FrozenStarlarkCmdArgs {
    items: ThinBoxSliceValue::empty(),
    hidden: ThinBoxSliceValue::empty(),
    options: FrozenCommandLineOptions::empty(),
});

impl<'v> Serialize for FrozenStarlarkCmdArgs<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        FieldsRef(self, PhantomData).serialize(serializer)
    }
}

impl<'a, 'v> Fields<'v> for Ref<'a, StarlarkCommandLineData<'v>> {
    fn items(&self) -> &[CommandLineArg<'v>] {
        &self.items
    }

    fn hidden(&self) -> &[CommandLineArg<'v>] {
        &self.hidden
    }

    fn options(&self) -> Option<&dyn CommandLineOptionsTrait<'v>> {
        match &self.options {
            None => None,
            Some(x) => Some(&**x),
        }
    }
}

impl<'v> Fields<'v> for FrozenStarlarkCmdArgs<'v> {
    fn items(&self) -> &[CommandLineArg<'v>] {
        CommandLineArg::slice_from_values_unchecked(&self.items)
    }

    fn hidden(&self) -> &[CommandLineArg<'v>] {
        CommandLineArg::slice_from_values_unchecked(&self.hidden)
    }

    fn options(&self) -> Option<&dyn CommandLineOptionsTrait<'v>> {
        if self.options.is_empty() {
            None
        } else {
            Some(&self.options)
        }
    }
}

impl<'v, F: Fields<'v>> Fields<'v> for &F {
    fn items(&self) -> &[CommandLineArg<'v>] {
        (*self).items()
    }

    fn hidden(&self) -> &[CommandLineArg<'v>] {
        (*self).hidden()
    }

    fn options(&self) -> Option<&dyn CommandLineOptionsTrait<'v>> {
        (*self).options()
    }
}

impl<'v, A: Fields<'v>, B: Fields<'v>> Fields<'v> for Either<A, B> {
    fn items(&self) -> &[CommandLineArg<'v>] {
        match self {
            Either::Left(x) => x.items(),
            Either::Right(x) => x.items(),
        }
    }

    fn hidden(&self) -> &[CommandLineArg<'v>] {
        match self {
            Either::Left(x) => x.hidden(),
            Either::Right(x) => x.hidden(),
        }
    }

    fn options(&self) -> Option<&dyn CommandLineOptionsTrait<'v>> {
        match self {
            Either::Left(x) => x.options(),
            Either::Right(x) => x.options(),
        }
    }
}

// These types show up a lot in the frozen heaps, so make sure they don't regress
size_assert::words_of_type!(StarlarkCmdArgs<'static>, 4);
size_assert::words_of_type!(FrozenStarlarkCmdArgs<'static>, 3);
size_assert::words_of_type!(CommandLineOptions<'static>, 10);

impl<'v> Display for StarlarkCmdArgs<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0.try_borrow() {
            Ok(x) => Display::fmt(&FieldsRef(x, PhantomData), f),
            Err(_) => write!(f, "<cmd_args borrowed>"),
        }
    }
}

impl<'v> Display for FrozenStarlarkCmdArgs<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&FieldsRef(self, PhantomData), f)
    }
}

impl<'v> StarlarkCommandLineData<'v> {
    fn options_mut(&mut self) -> &mut CommandLineOptions<'v> {
        if self.options.is_none() {
            self.options = Some(Box::default());
        }
        self.options.as_mut().unwrap()
    }
}

impl<'v> StarlarkCmdArgs<'v> {
    pub(crate) fn is_concat(&self) -> bool {
        FieldsRef(self.0.borrow(), PhantomData).is_concat()
    }
}

impl<'v> FrozenStarlarkCmdArgs<'v> {
    pub(crate) fn is_concat(&self) -> bool {
        FieldsRef(self, PhantomData).is_concat()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<'v> StarlarkCmdArgs<'v> {
    pub fn is_empty(&self) -> bool {
        self.0.borrow().items.is_empty()
    }
}

/// A `cmd_args` in either of its representations: the receiver of the type's methods, and the
/// binding for a native parameter of type `cmd_args`.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum StarlarkCmdArgsView<'v> {
    Unfrozen(ValueTyped<'v, StarlarkCmdArgs<'v>>),
    Frozen(ValueTyped<'v, FrozenStarlarkCmdArgs<'v>>),
}

impl<'v> StarlarkCmdArgsView<'v> {
    pub fn is_empty(&self) -> bool {
        match self {
            StarlarkCmdArgsView::Unfrozen(x) => x.is_empty(),
            StarlarkCmdArgsView::Frozen(x) => x.is_empty(),
        }
    }

    pub fn as_command_line_arg_like(self) -> &'v dyn CommandLineArgLike<'v> {
        match self {
            StarlarkCmdArgsView::Unfrozen(x) => x.as_ref(),
            StarlarkCmdArgsView::Frozen(x) => x.as_ref(),
        }
    }
}

starlark::methods_static!(CMD_ARGS_METHODS = cmd_args_methods);

#[starlark_value(type = "cmd_args")]
impl<'v> StarlarkType<'v> for StarlarkCmdArgsView<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(CMD_ARGS_METHODS.methods())
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkCmdArgs<'v> {
    type Type = StarlarkCmdArgsView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenStarlarkCmdArgs<'v> {
    type Type = StarlarkCmdArgsView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike<'v>>(self);
    }
}

impl<'v> AllocValue<'v> for StarlarkCmdArgs<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

impl<'v> CommandLineArgLike<'v> for StarlarkCmdArgs<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkCmdArgs::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        FieldsRef(self.0.borrow(), PhantomData).add_to_command_line(fmt)
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        FieldsRef(self.0.borrow(), PhantomData).visit_artifacts(visitor)
    }

    fn contains_arg_attr(&self) -> bool {
        FieldsRef(self.0.borrow(), PhantomData).contains_arg_attr()
    }

    fn visit_write_to_file_macros(
        &self,
        visitor: &mut dyn WriteToFileMacroVisitor,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        FieldsRef(self.0.borrow(), PhantomData)
            .visit_write_to_file_macros(visitor, artifact_path_mapping)
    }
}

impl<'v> CommandLineArgLike<'v> for FrozenStarlarkCmdArgs<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(FrozenStarlarkCmdArgs::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        FieldsRef(self, PhantomData).add_to_command_line(fmt)
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        FieldsRef(self, PhantomData).visit_artifacts(visitor)
    }

    fn contains_arg_attr(&self) -> bool {
        FieldsRef(self, PhantomData).contains_arg_attr()
    }

    fn visit_write_to_file_macros(
        &self,
        visitor: &mut dyn WriteToFileMacroVisitor,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        FieldsRef(self, PhantomData).visit_write_to_file_macros(visitor, artifact_path_mapping)
    }
}

/// The direct freeze plan reusing the static empty `cmd_args`.
///
/// A free function: naming the static's rebrand inside the early-bound
/// `prepare_freeze` trait method trips a spurious rustc bound failure; the
/// identical expression resolves here.
fn empty_cmd_args_plan<'v, 'fv>() -> StaticFreezePlan<'v, 'fv, StarlarkCmdArgs<'v>> {
    StaticFreezePlan::direct(EMPTY_FROZEN_CMD_ARGS.at())
}

impl<'v> Freeze<'v> for StarlarkCmdArgs<'v> {
    type Frozen<'fv> = FrozenStarlarkCmdArgs<'fv>;

    fn prepare_freeze<'fv>(
        &self,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<StaticFreezePlan<'v, 'fv, Self>>
    where
        Self::Frozen<'fv>: StarlarkValue<'fv>,
    {
        let StarlarkCommandLineData {
            items,
            hidden,
            options,
        } = &*self.0.borrow();
        if items.is_empty() && hidden.is_empty() && options.is_none() {
            Ok(empty_cmd_args_plan())
        } else {
            Ok(StaticFreezePlan::allocate())
        }
    }

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let StarlarkCommandLineData {
            items,
            hidden,
            options,
        } = self.0.into_inner();

        fn freeze_elements<'v, 'fv>(
            elements: MiniVec<CommandLineArg<'v>>,
            freezer: &Freezer<'v, 'fv>,
        ) -> FreezeResult<ThinBoxSliceValue<'fv>> {
            elements
                .into_iter()
                .map(|x| Ok(x.freeze(freezer)?.to_value()))
                .collect()
        }
        let items = freeze_elements(items, freezer)?;
        let hidden = freeze_elements(hidden, freezer)?;
        let options = options
            .map(|options| Freeze::freeze(*options, freezer))
            .transpose()?
            .unwrap_or_default();

        Ok(FrozenStarlarkCmdArgs {
            items,
            hidden,
            options,
        })
    }
}

impl<'v> StarlarkCmdArgs<'v> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub fn try_from_value(value: Value<'v>) -> buck2_error::Result<Self> {
        Self::try_from_value_typed(StarlarkCommandLineValueUnpack::unpack_value_err(value)?)
    }

    pub fn try_from_value_typed(
        value: StarlarkCommandLineValueUnpack<'v>,
    ) -> buck2_error::Result<Self> {
        let mut builder = Self::new();
        builder.0.get_mut().add_value_typed(value)?;
        Ok(builder)
    }
}

#[derive(UnpackValue, StarlarkTypeRepr)]
pub enum StarlarkCommandLineValueUnpack<'v> {
    // This should be `list[Self]`, but we cannot express it.
    List(&'v ListRef<'v>),
    CommandLineArg(CommandLineArg<'v>),
}

impl<'v> StarlarkCommandLineData<'v> {
    fn add_value(&mut self, value: Value<'v>) -> buck2_error::Result<()> {
        self.add_value_typed(StarlarkCommandLineValueUnpack::unpack_value_err(value)?)
    }

    fn add_value_typed(
        &mut self,
        value: StarlarkCommandLineValueUnpack<'v>,
    ) -> buck2_error::Result<()> {
        match value {
            StarlarkCommandLineValueUnpack::List(values) => self.add_values(values.content())?,
            StarlarkCommandLineValueUnpack::CommandLineArg(value) => self.items.push(value),
        }
        Ok(())
    }

    /// Check the types of a list of values, and modify `data` accordingly
    ///
    /// The values must be one of: CommandLineArgLike or a list thereof.
    fn add_values(&mut self, values: &[Value<'v>]) -> buck2_error::Result<()> {
        self.items.reserve(values.len());
        for value in values {
            self.add_value(*value)?
        }
        Ok(())
    }

    fn add_from_iterator(
        &mut self,
        values: impl Iterator<Item = Value<'v>>,
    ) -> buck2_error::Result<()> {
        let (lower, upper) = values.size_hint();
        self.items.reserve(upper.unwrap_or(lower));
        values
            .into_iter()
            .try_for_each(|value| self.add_value(value))?;
        Ok(())
    }

    /// Add values to the artifact that don't show up on the command line, but do for dependency
    fn add_hidden(&mut self, value: StarlarkCommandLineValueUnpack<'v>) -> buck2_error::Result<()> {
        match value {
            StarlarkCommandLineValueUnpack::List(values) => {
                for value in values.content() {
                    self.add_hidden(StarlarkCommandLineValueUnpack::unpack_value_err(*value)?)?
                }
            }
            StarlarkCommandLineValueUnpack::CommandLineArg(arg) => {
                self.hidden.push(arg);
            }
        }
        Ok(())
    }
}

struct StarlarkCommandLineMut<'v> {
    value: Value<'v>,
    borrow: RefMut<'v, StarlarkCommandLineData<'v>>,
}

impl<'v> StarlarkTypeRepr for StarlarkCommandLineMut<'v> {
    type Canonical = <StarlarkCmdArgs<'v> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        StarlarkCmdArgs::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for StarlarkCommandLineMut<'v> {
    type Error = starlark::Error;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        match StarlarkCmdArgsView::unpack_value_opt(value) {
            None => Ok(None),
            Some(StarlarkCmdArgsView::Unfrozen(v)) => Ok(Some(Self {
                value,
                borrow: v.as_ref().0.borrow_mut(),
            })),
            // A `cmd_args` all the same; what fails is the mutation.
            Some(StarlarkCmdArgsView::Frozen(_)) => Err(starlark::Error::new_other(
                ValueError::CannotMutateImmutableValue,
            )),
        }
    }
}

impl<'v> AllocValue<'v> for StarlarkCommandLineMut<'v> {
    fn alloc_value(self, _heap: Heap<'v>) -> Value<'v> {
        self.value
    }
}

fn cmd_args<'v>(x: Value<'v>) -> FieldsRef<'v, impl Fields<'v>> {
    if let Some(x) = x.downcast_ref::<StarlarkCmdArgs>() {
        FieldsRef(Either::Left(x.0.borrow()), PhantomData)
    } else if let Some(x) = x.downcast_ref::<FrozenStarlarkCmdArgs>() {
        FieldsRef(Either::Right(x), PhantomData)
    } else {
        unreachable!("This parameter must always be a type of command args")
    }
}

/// The `cmd_args` type is created by `cmd_args()` and is consumed by `ctx.actions.run`.
/// The type is a mutable collection of strings and `artifact` values.
/// In general, command lines, artifacts, strings, `RunInfo` and lists thereof can be added to or used to construct a `cmd_args` value.
/// All these methods operate mutably on `cmd` and return that value too.
// TODO(nga): `cmd_args` should be immutable, so that all parameters should be
//   either set in constructor, or operations like `hidden` should return a copy
//   rather than modify this. https://fburl.com/workplace/ihkplvbn
#[starlark_module]
fn cmd_args_methods(builder: &mut MethodsBuilder) {
    /// A list of arguments to be added to the command line, which may including `cmd_args`, artifacts, strings, `RunInfo` or lists thereof.
    /// Note that this operation mutates the input `cmd_args`.
    fn add<'v>(
        mut this: StarlarkCommandLineMut<'v>,
        heap: Heap<'v>,
        args: &Arguments<'v, '_>,
    ) -> starlark::Result<StarlarkCommandLineMut<'v>> {
        args.no_named_args()?;
        let values = args.positions(heap)?;
        this.borrow.add_from_iterator(values)?;
        Ok(this)
    }

    /// Make all artifact paths relative to a given location. Typically used when the command
    /// you are running changes directory.
    ///
    /// By default, the paths are relative to the artifacts themselves (equivalent to
    /// `parent = 0`). Use `parent` to make the paths relative to an ancestor directory.
    /// For example `parent = 1` would make all paths relative to the containing dirs
    /// of any artifacts in the `cmd_args`.
    ///
    /// ```python
    /// dir = symlinked_dir(...)
    /// script = [
    ///     cmd_args(cmd_args(dir, format = "cd {}"),
    ///     original_script.relative_to(dir)
    /// ]
    /// ```
    fn relative_to<'v>(
        mut this: StarlarkCommandLineMut<'v>,
        #[starlark(require = pos)] directory: ValueOf<'v, RelativeOrigin<'v>>,
        #[starlark(require = named, default = 0u32)] parent: u32,
    ) -> starlark::Result<StarlarkCommandLineMut<'v>> {
        this.borrow.options_mut().relative_to = Some((directory, parent));
        Ok(this)
    }

    /// Returns a copy of the `cmd_args` such that any modifications to the original or the returned value will not impact each other.
    /// Note that this is a shallow copy, so any inner `cmd_args` can still be modified.
    fn copy<'v>(this: Value<'v>) -> starlark::Result<StarlarkCmdArgs<'v>> {
        Ok(cmd_args(this).copy())
    }

    /// Collect all the inputs (including hidden) referenced by this command line.
    ///
    /// The returned collection is opaque and primarily useful for:
    /// - Checking if the command has any artifact dependencies
    /// - Comparing input sets between different `cmd_args` objects
    ///
    /// The collection supports `len()` and equality comparisons but cannot be iterated.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     cmd = cmd_args(ctx.attrs.srcs)
    ///
    ///     # Check if command has any inputs
    ///     if len(cmd.inputs) > 0:
    ///         pass
    ///
    ///     # Compare input sets
    ///     other_cmd = cmd_args(ctx.attrs.headers, hidden = ctx.attrs.resources)
    ///     if cmd.inputs == other_cmd.inputs:
    ///         pass
    /// ```
    #[starlark(attribute)]
    fn inputs<'v>(this: Value<'v>) -> starlark::Result<StarlarkCommandLineInputs> {
        let mut visitor = SimpleCommandLineArtifactVisitor::new();
        cmd_args(this).visit_artifacts(&mut visitor)?;
        Ok(StarlarkCommandLineInputs {
            inputs: visitor.inputs,
        })
    }

    /// Collect all the outputs (including hidden) referenced by this command line.
    #[starlark(attribute)]
    fn outputs<'v>(
        this: Value<'v>,
        heap: Heap<'_>,
    ) -> starlark::Result<Vec<StarlarkOutputArtifact<'v>>> {
        let mut visitor = SimpleCommandLineArtifactVisitor::new();
        cmd_args(this).visit_artifacts(&mut visitor)?;
        let mut outputs =
            Vec::with_capacity(visitor.declared_outputs.len() + visitor.frozen_outputs.len());
        for out in visitor.declared_outputs {
            let declared = heap.alloc_typed(StarlarkDeclaredArtifact::new(
                None,
                (*out).dupe(),
                AssociatedArtifacts::new(),
            ));
            outputs.push(StarlarkOutputArtifact::new(declared));
        }
        // FIXME(JakobDegen): We should probably not be allowing people to get an `OutputArtifact`
        // for an artifact declared in a downstream action??
        for out in visitor.frozen_outputs {
            let declared = heap.alloc_typed(StarlarkDeclaredArtifact::new(
                None,
                (*out
                    .allocate_new_output_artifact_for(heap)
                    .internal_error("Expecting artifact to be output artifact")?)
                .dupe(),
                AssociatedArtifacts::new(),
            ));
            outputs.push(StarlarkOutputArtifact::new(declared));
        }
        Ok(outputs)
    }
}

#[starlark_module]
pub fn register_cmd_args(builder: &mut GlobalsBuilder) {
    #[starlark(as_type = FrozenStarlarkCmdArgs)]
    /// The `cmd_args` type is created by this function and is consumed by `ctx.actions.run`.
    /// The type is a mutable collection of strings and artifact values.
    /// In general, command lines, artifacts, strings, `RunInfo` and lists thereof can be added to or used to construct a `cmd_args` value.
    ///
    /// The arguments are:
    ///
    /// * `*args` - a list of things to add to the command line, each of which must be coercible to a command line. Further items can be added with `cmd.add`.
    /// * `format` - a string that provides a format to apply to the argument. for example, `cmd_args(x, format="--args={}")` would prepend `--args=` before `x`, or if `x` was a list, before each element in `x`.
    /// * `delimiter` - added between arguments to join them together. For example, `cmd_args(["--args=",x], delimiter="")` would produce a single argument to the underlying tool.
    /// * `prepend` - added as a separate argument before each argument.
    /// * `quote` - indicates whether quoting is to be applied to each argument. The only current valid value is `"shell"`.
    /// * `ignore_artifacts` - if `True`, artifacts contribute only their paths: nothing is materialized for them. Artifacts with content-based-paths are built, so that the content hash embedded in the path can be rendered.
    /// * `hidden` - artifacts not present on the command line, but added as dependencies.
    /// * `absolute_prefix` and `absolute_suffix` - added to the start and end of each artifact.
    /// * `parent` - for all the artifacts use their `parent`th directory (e.g. `parent = 1` for the directory the artifact is located, `parent = 2` for that directory's parent, etc.).
    /// * `relative_to` - make all artifact paths relative to a given location.
    /// * `replace_regex` - replaces arguments with a regular expression.
    ///
    /// ### `ignore_artifacts`
    ///
    /// `ignore_artifacts=True` declares that the command records the paths of the referenced
    /// artifacts but never reads their contents, so none of them is materialized for it.
    /// An artifact whose path is independent of its contents introduces no dependency at all.
    /// An artifact whose path embeds its content hash is built before the command runs,
    /// since the path cannot be rendered otherwise, but is still not materialized; its
    /// content reaches the command only through the rendered path.
    ///
    /// Artifacts in the `hidden` list of such a scope still introduce no dependency, and
    /// a content-based one there is an error: nothing renders it, so nothing would carry
    /// its content to the action.
    ///
    /// As an example where this can be useful, consider passing a dependency that is only accessed at runtime, but whose path
    /// must be baked into the binary. As an example:
    ///
    /// ```python
    /// resources = cmd_args(resource_file, format = "-DFOO={}", ignore_artifacts=True)
    /// ctx.actions.run(cmd_args("gcc", "-c", source_file, resources))
    /// ```
    ///
    /// Note that `ignore_artifacts` sets all artifacts referenced by this `cmd_args` to be ignored, including those added afterwards,
    /// so generally create a special `cmd_args` and scope it quite tightly.
    ///
    /// If you actually do use the inputs referenced by this command,
    /// you will either error out due to missing dependencies (if running actions remotely)
    /// or have untracked dependencies that will fail to rebuild when it should.
    ///
    /// ### `hidden`
    ///
    /// Things to add to the command line which do not show up but are added as dependencies.
    /// The values can be anything normally permissible to pass to `add`.
    ///
    /// Typically used if the command you are running implicitly depends on files that are not
    /// passed on the command line, e.g. headers in the case of a C compilation.
    ///
    /// ### `absolute_prefix` and `absolute_suffix`
    ///
    /// Adds a prefix to the start or end of every artifact.
    ///
    /// Prefix is often used if you have a `$ROOT` variable
    /// in a shell script and want to use it to make files absolute.
    ///
    /// Suffix is often used in conjunction with `absolute_prefix`
    /// to wrap artifacts in function calls.
    ///
    /// ```python
    /// cmd_args(script, absolute_prefix = "$ROOT/")
    /// cmd_args(script, absolute_prefix = "call", absolute_suffix = ")")
    /// ```
    ///
    /// ### `parent`
    ///
    /// For all the artifacts use their parent directory.
    ///
    /// Typically used when the file name is passed one way, and the directory another,
    /// e.g. `cmd_args(artifact, format="-L{}", parent=1)`.
    ///
    /// ### `relative_to=dir` or `relative_to=(dir, parent)`
    ///
    /// Make all artifact paths relative to a given location. Typically used when the command
    /// you are running changes directory.
    ///
    /// By default, the paths are relative to the artifacts themselves (equivalent to
    /// parent equals to `0`). Use `parent` to make the paths relative to an ancestor directory.
    /// For example parent equals to `1` would make all paths relative to the containing dirs
    /// of any artifacts in the `cmd_args`.
    ///
    /// ```python
    /// dir = symlinked_dir(...)
    /// script = [
    ///     cmd_args(dir, format = "cd {}", relative_to=dir),
    /// ]
    /// ```
    ///
    /// ### `replace_regex`
    ///
    /// Replaces all parts matching pattern regular expression (or regular expressions)
    /// in each argument with replacement strings.
    fn cmd_args<'v>(
        #[starlark(args)] args: ValueOf<'v, TupleType<StarlarkCommandLineValueUnpack<'v>>>,
        hidden: Option<StarlarkCommandLineValueUnpack<'v>>,
        delimiter: Option<StringValue<'v>>,
        format: Option<StringValue<'v>>,
        prepend: Option<StringValue<'v>>,
        quote: Option<&str>,
        #[starlark(default = false)] ignore_artifacts: bool,
        #[starlark(default = false, require = named, doc_hidden = true)]
        experimental_salt_hidden: bool,
        absolute_prefix: Option<StringValue<'v>>,
        absolute_suffix: Option<StringValue<'v>>,
        #[starlark(default = 0)] parent: u32,
        relative_to: Option<
            Either<ValueOf<'v, RelativeOrigin<'v>>, (ValueOf<'v, RelativeOrigin<'v>>, u32)>,
        >,
        #[starlark(default = Either::Right(ValueOf::empty_list()))] replace_regex: Either<
            (CmdArgsRegex<'v>, StringValue<'v>),
            ValueOf<'v, ListType<(CmdArgsRegex<'v>, StringValue<'v>)>>,
        >,
    ) -> starlark::Result<StarlarkCmdArgs<'v>> {
        let format = format.filter(|format| format.as_str() != "{}");
        let quote = quote.map(QuoteStyle::parse).transpose()?;
        let mut builder = StarlarkCommandLineData::default();
        if delimiter.is_some()
            || format.is_some()
            || prepend.is_some()
            || quote.is_some()
            || ignore_artifacts
            || experimental_salt_hidden
            || absolute_prefix.is_some()
            || absolute_suffix.is_some()
            || parent != 0
            || relative_to.is_some()
        {
            let opts = builder.options_mut();
            opts.delimiter = delimiter;
            opts.format = format;
            opts.prepend = prepend;
            opts.quote = quote;
            opts.ignore_artifacts = ignore_artifacts;
            opts.salt_hidden = experimental_salt_hidden;
            opts.absolute_prefix = absolute_prefix;
            opts.absolute_suffix = absolute_suffix;
            opts.parent = parent;
            opts.relative_to = relative_to.map(|either| {
                let (relative_to, parent) = either.map_left(|o| (o, 0)).into_inner();
                (relative_to, parent)
            });
        }
        let replace_regex: Vec<(CmdArgsRegex, StringValue)> = match replace_regex {
            Either::Left(x) => vec![x],
            Either::Right(xs) => xs.try_to_vec()?,
        };
        if !replace_regex.is_empty() {
            for (pattern, _replacement) in &replace_regex {
                pattern.validate()?;
            }
            builder.options_mut().replacements = replace_regex.into();
        }
        for v in args.items() {
            builder.add_value_typed(v)?;
        }
        if let Some(hidden) = hidden {
            builder.add_hidden(hidden)?;
        }
        Ok(StarlarkCmdArgs(RefCell::new(builder)))
    }
}

/// A wrapper for a [StarlarkCmdArgs]'s inputs. This is an opaque type that only allows
/// debug-printing and querying the length to tell if any inputs exist.
#[derive(
    Debug,
    PartialEq,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
pub struct StarlarkCommandLineInputs {
    #[starlark_pagable(pagable)]
    pub inputs: BuckMutInsertionSet<ArtifactGroup>,
}

starlark_simple_value!(StarlarkCommandLineInputs);

impl Display for StarlarkCommandLineInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt_container(f, "command_line_inputs(", ")", self.inputs.iter())
    }
}

starlark::methods_static!(COMMAND_LINE_INPUTS_METHODS = command_line_inputs_methods);

#[starlark_value(type = "CommandLineInputs")]
impl<'v> StarlarkValue<'v> for StarlarkCommandLineInputs {
    fn get_methods() -> Option<&'static Methods> {
        Some(COMMAND_LINE_INPUTS_METHODS.methods())
    }

    fn length(&self) -> starlark::Result<i32> {
        self.inputs
            .len()
            .try_into()
            .map_err(starlark::Error::new_other)
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        if let Some(other) = other.downcast_ref::<Self>() {
            Ok(self.inputs.eq_unordered(&other.inputs))
        } else {
            Ok(false)
        }
    }
}

/// An opaque collection of input artifacts referenced by a `cmd_args` object.
///
/// Returned by the [`.inputs`](../cmd_args/#cmd_argsinputs) attribute. Supports `len()` and equality comparisons.
/// See the [`.inputs`](../cmd_args/#cmd_argsinputs) attribute documentation for usage examples.
#[starlark_module]
fn command_line_inputs_methods(_builder: &mut MethodsBuilder) {
    // No methods currently - this type only supports len() and equality via StarlarkValue trait
}

#[starlark_module]
#[starlark_types(
    StarlarkCommandLineInputs as CommandLineInputs
)]
pub(crate) fn register_command_line_inputs(globals: &mut GlobalsBuilder) {}
