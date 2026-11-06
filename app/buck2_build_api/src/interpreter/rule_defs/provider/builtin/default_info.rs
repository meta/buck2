/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::iter;
use std::ptr;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_build_api_derive::internal_provider;
use buck2_hash::BuckMutInsertionMap;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FrozenHeap;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::AllocDict;
use starlark::values::dict::DictType;
use starlark::values::list::AllocList;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;

use crate as buck2_build_api;
use crate::artifact_groups::ArtifactGroup;
use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use crate::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use crate::interpreter::rule_defs::provider::ProviderCollection;

/// A provider that all rules' implementations must return
///
/// In many simple cases, this can be inferred for the user.
///
/// Example of a rule's implementation function and how these fields are used by the framework:
///
/// ```python
/// # //foo_binary.bzl
/// def impl(ctx):
///     out = ctx.actions.declare_output("out")
///     ctx.actions.run([ctx.attrs._cc[RunInfo], "-o", out.as_output()] + ctx.attrs.srcs)
///     stripped_out = ctx.actions.declare_output("stripped")
///     debug_symbols_out = ctx.actions.declare_output("debug_info")
///     ctx.actions.run([
///         ctx.attrs._strip[RunInfo],
///         "--binary",
///         out,
///         "--stripped-out",
///         stripped_out.as_output(),
///         "--debug-symbols-out",
///         debug_symbols_out.as_output(),
///     ])
///     return [
///         DefaultInfo(
///             sub_targets = {
///                 "stripped": [
///                     DefaultInfo(default_outputs = [stripped_out, debug_symbols_out]),
///                 ],
///             },
///             default_output = out,
///         ),
///     ]
///
/// foo_binary = rule(
///     impl = impl,
///     attrs = {
///         "srcs": attrs.list(attrs.source()),
///         "_cc": attrs.dep(default = "//tools:cc", providers = [RunInfo]),
///         "_strip_script": attrs.dep(default = "//tools:strip", providers = [RunInfo]),
///     },
/// )
///
///
/// def foo_binary_wrapper(name, srcs):
///     foo_binary(
///         name = name,
///         srcs = src,
///         out = name,
///         stripped = name + ".stripped",
///         debug_info = name + ".debug_info",
///     )
///
/// # //subdir/BUCK
/// load("//:foo_binary.bzl", "foo_binary_wrapper")
///
/// genrule(name = "gen_stuff", ...., default_outs = ["foo.cpp"])
///
/// # ":gen_stuff" pulls the default_outputs for //subdir:gen_stuff
/// foo_binary_wrapper(name = "foo", srcs = glob(["*.cpp"]) + [":gen_stuff"])
///
/// # Builds just 'foo' binary. The strip command is never invoked.
/// $ buck build //subdir:foo
///
/// # builds the 'foo' binary, because it is needed by the 'strip' command. Ensures that
/// # both the stripped binary and the debug symbols are built.
/// $ buck build //subdir:foo[stripped]
/// ```
#[internal_provider(default_info_creator)]
#[derive(
    Clone,
    Debug,
    Freeze,
    Trace,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[freeze(validator = validate_default_info)]
#[repr(C)]
pub struct DefaultInfo<'v> {
    /// A mapping of names to `ProviderCollection`s. The keys are used when resolving the
    /// `ProviderName` portion of a `ProvidersLabel` in order to access the providers for a
    /// subtarget, such as when doing `buck2 build cell//foo:bar[baz]`. Just like any
    /// `ProviderCollection`, this collection must include at least a `DefaultInfo` provider. The
    /// subtargets can have their own subtargets as well, which can be accessed by chaining them,
    /// e.g.: `buck2 build cell//foo:bar[baz][qux]`.
    sub_targets: ValueOf<'v, DictType<StringValue<'v>, ValueTyped<'v, ProviderCollection<'v>>>>,
    /// A list of `Artifact`s that are built by default if this rule is requested
    /// explicitly (via CLI or `$(location)` etc), or depended on as as a "source"
    /// (i.e., `attrs.source()`).
    default_outputs: ValueOf<'v, ListType<StarlarkArtifactView<'v>>>,
    /// A list of `ArtifactTraversable`. The underlying `Artifact`s they define will
    /// be built by default if this rule is requested (via CLI or `$(location)` etc),
    /// but _not_ when it's depended on as as a "source" (i.e., `attrs.source()`).
    /// `ArtifactTraversable` can be an `Artifact` (which yields itself), or
    /// `cmd_args`, which expand to all their inputs.
    other_outputs: ValueOf<'v, ListType<ValueAsCommandLineLike<'v>>>,
}

fn validate_default_info<'v>(info: &DefaultInfo<'v>) -> buck2_error::Result<()> {
    if info.default_outputs.list().len() > 1 {
        tracing::info!("DefaultInfo.default_output should only have a maximum of 1 item.");
        // TODO use soft_error when landed
        // TODO error rather than soft warning
        // return Err(buck2_error::buck2_error!(
        //     "DefaultInfo.default_output can only have a maximum of 1 item."
        // ));
    }
    Ok(())
}

impl<'v> DefaultInfo<'v> {
    pub fn empty(heap: Heap<'v>) -> Self {
        DefaultInfo {
            sub_targets: ValueOf::alloc_dict(heap, []),
            default_outputs: ValueOf::alloc_list(heap, []),
            other_outputs: ValueOf::alloc_list(heap, []),
        }
    }

    pub(crate) fn testing_empty(heap: FrozenHeap<'v>) -> ValueTyped<'v, DefaultInfo<'v>> {
        heap.alloc_simple_typed(DefaultInfo {
            sub_targets: ValueOf::new_unchecked(
                heap.alloc(AllocDict(iter::empty::<(String, ProviderCollection)>())),
            ),
            default_outputs: ValueOf::new_unchecked(heap.alloc(AllocList::EMPTY)),
            other_outputs: ValueOf::new_unchecked(heap.alloc(AllocList::EMPTY)),
        })
    }

    pub fn get_sub_target_providers(
        &self,
        name: &str,
    ) -> buck2_error::Result<Option<ValueTyped<'v, ProviderCollection<'v>>>> {
        Ok(self.sub_targets.get_str(name)?.map(|v| v.unpack()))
    }

    fn default_outputs_impl(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<StarlarkArtifact>> + 'v {
        self.default_outputs
            .iter()
            .map(|v| -> buck2_error::Result<StarlarkArtifact> {
                v?.unpack()
                    .as_input_artifact_like()
                    .get_bound_starlark_artifact()
            })
    }

    pub fn default_outputs(&self) -> buck2_error::Result<Vec<StarlarkArtifact>> {
        self.default_outputs_impl().collect()
    }

    pub fn default_outputs_raw(&self) -> Value<'v> {
        self.default_outputs.to_value()
    }

    pub fn sub_targets(
        &self,
    ) -> buck2_error::Result<BuckMutInsertionMap<&'v str, ValueTyped<'v, ProviderCollection<'v>>>>
    {
        self.sub_targets
            .iter()
            .map(|entry| {
                let (k, v) = entry?;
                Ok((k.unpack().as_str(), v.unpack()))
            })
            .collect()
    }

    pub fn sub_targets_raw(&self) -> Value<'v> {
        self.sub_targets.to_value()
    }

    pub fn for_each_default_output_artifact_only(
        &self,
        processor: &mut dyn FnMut(Artifact),
    ) -> buck2_error::Result<()> {
        for output in self.default_outputs.iter() {
            processor(
                output?
                    .unpack()
                    .as_input_artifact_like()
                    .get_bound_artifact()?,
            );
        }
        Ok(())
    }

    pub fn for_each_default_output_other_artifacts_only(
        &self,
        processor: &mut dyn FnMut(ArtifactGroup),
    ) -> buck2_error::Result<()> {
        for output in self.default_outputs.iter() {
            let others = output?
                .unpack()
                .as_input_artifact_like()
                .get_associated_artifacts();
            others
                .iter()
                .flat_map(|v| v.iter())
                .for_each(|other| processor(other.dupe()));
        }
        Ok(())
    }

    pub fn for_each_other_output(
        &self,
        processor: &mut dyn FnMut(ArtifactGroup),
    ) -> buck2_error::Result<()> {
        struct Visitor<'x>(&'x mut dyn FnMut(ArtifactGroup));

        impl<'v> CommandLineArtifactVisitor<'v> for Visitor<'_> {
            fn visit_input(
                &mut self,
                input: ArtifactGroup,
                _: InputProperties,
                _: Vec<&ArtifactTag>,
            ) {
                (self.0)(input);
            }

            fn visit_declared_output(
                &mut self,
                _artifact: OutputArtifact<'v>,
                _tags: Vec<&ArtifactTag>,
            ) {
            }

            fn visit_frozen_output(&mut self, _artifact: Artifact, _tags: Vec<&ArtifactTag>) {}
        }

        for output in self.other_outputs.iter() {
            output?
                .unpack()
                .0
                .visit_artifacts(&mut Visitor(processor))?;
        }
        Ok(())
    }

    pub fn for_each_output(
        &self,
        processor: &mut dyn FnMut(ArtifactGroup),
    ) -> buck2_error::Result<()> {
        self.for_each_default_output_artifact_only(&mut |a| processor(ArtifactGroup::Artifact(a)))?;
        self.for_each_default_output_other_artifacts_only(processor)?;
        self.for_each_other_output(processor)
    }
}

impl<'v> PartialEq for DefaultInfo<'v> {
    // frozen default infos can be compared by ptr for a simple equality
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum DefaultOutputError {
    #[error("Cannot specify both `default_output` and `default_outputs`.")]
    ConflictingArguments,
}

#[starlark_module]
fn default_info_creator(builder: &mut GlobalsBuilder) {
    #[starlark(as_type = DefaultInfo<'static>)]
    fn DefaultInfo<'v>(
        // TODO(nga): parameters must be named only.
        #[starlark(default = NoneOr::None)] default_output: NoneOr<
            ValueOf<'v, StarlarkArtifactView<'v>>,
        >,
        #[starlark(default = NoneOr::None)] default_outputs: NoneOr<
            ValueOf<'v, ListType<StarlarkArtifactView<'v>>>,
        >,
        #[starlark(default = ValueOf::new_unchecked(Value::new_empty_list()))]
        other_outputs: ValueOf<'v, ListType<ValueAsCommandLineLike<'v>>>,
        #[starlark(default = ValueOf::empty_dict())] sub_targets: ValueOf<
            'v,
            DictType<StringValue<'v>, Value<'v>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<DefaultInfo<'v>> {
        let heap = eval.heap();

        // support both list and singular options for now until we migrate all the rules.
        let default_outputs = match (default_outputs.into_option(), default_output.into_option()) {
            (Some(list), None) => list,
            (None, Some(default_output)) => ValueOf::alloc_list(heap, [default_output]),
            (None, None) => ValueOf::alloc_list(heap, []),
            (Some(_), Some(_)) => {
                return Err(
                    buck2_error::Error::from(DefaultOutputError::ConflictingArguments).into(),
                );
            }
        };

        let sub_targets = sub_targets
            .try_items()
            .map(|entry| {
                let (k, v) = entry?;
                let collection = ProviderCollection::try_from_value_subtarget(v, heap)?;
                buck2_error::Ok((
                    ValueOf::from(k),
                    ValueOf::from(heap.alloc_typed(collection)),
                ))
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;

        Ok(DefaultInfo {
            default_outputs,
            other_outputs,
            sub_targets: ValueOf::alloc_dict(heap, sub_targets),
        })
    }
}
