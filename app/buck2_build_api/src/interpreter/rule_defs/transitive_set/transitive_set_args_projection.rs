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
use std::fmt::Display;
use std::iter;
use std::sync::Arc;

use allocative::Allocative;
use buck2_error::BuckErrorOptionContext;
use display_container::display_pair;
use display_container::fmt_container;
use display_container::iter_display_chain;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::list::ListRef;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::TransitiveSetProjectionKey;
use crate::artifact_groups::TransitiveSetProjectionWrapper;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use crate::interpreter::rule_defs::transitive_set::TransitiveSet;
use crate::interpreter::rule_defs::transitive_set::traversal::TransitiveSetOrdering;
use crate::interpreter::rule_defs::transitive_set::traversal::TransitiveSetProjectionTraversal;

/// TransitiveSetArgsProjection is the starlark value returned from the starlark method `transitive_set.project_as_args()`
///
/// The projected values are all stored on the TransitiveSet itself and this value will reference back to that. The main
/// point of this object is to provide the implementation of CommandLineArgLike so that the args projection
/// can be used in places that accept command lines.
#[derive(
    Debug,
    Clone,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[derive(NoSerialize)] // TODO we should probably have a serialization for transitive set
#[repr(C)]
pub struct TransitiveSetArgsProjection<'v> {
    pub(super) transitive_set: ValueTyped<'v, TransitiveSet<'v>>,

    /// The index of the projection. Once transitive sets are defined, their projections never
    /// change, so we can afford to just store the index here.
    pub projection: usize,

    /// The ordering to use when traversing the projection.
    pub ordering: TransitiveSetOrdering,
}

impl<'v> Display for TransitiveSetArgsProjection<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let projection_name = self.projection_name().unwrap_or("<invalid projection>");
        fmt_container(
            f,
            "TransitiveSetProjection(",
            ")",
            iter_display_chain(
                iter::once(projection_name),
                iter::once(display_pair("TransitiveSet", "=", &self.transitive_set)),
            ),
        )
    }
}

impl<'v> TransitiveSetArgsProjection<'v> {
    fn projection_name(&self) -> buck2_error::Result<&'v str> {
        self.transitive_set
            .as_ref()
            .projection_name(self.projection)
    }
}

impl<'v> TransitiveSetArgsProjection<'v> {
    /// For args projections, we allow either a CommandLineArgLike or a list of them (just like `cmd_args.add()`).
    /// This function allows us to treat those two as the same.
    /// TODO(cjhopman): It may be better to wrap the list case in a new CommandLineArgLike impl when returned from
    /// the projection. Then we'd only have to verify the contents type once and it might be a bit simpler to use.
    pub(super) fn as_command_line(
        v: Value<'v>,
    ) -> buck2_error::Result<impl CommandLineArgLike<'v> + 'v> {
        enum Impl<'v> {
            Item(&'v dyn CommandLineArgLike<'v>),
            List(&'v [Value<'v>]),
        }
        impl<'v> CommandLineArgLike<'v> for Impl<'v> {
            fn register_me(&self) {
                // No need because this is not proper implementation.
            }

            fn add_to_command_line(
                &self,
                fmt: &mut CommandLineBuilder<'v, '_>,
            ) -> buck2_error::Result<()> {
                match self {
                    Impl::Item(v) => v.add_to_command_line(fmt),
                    Impl::List(items) => {
                        for v in *items {
                            ValueAsCommandLineLike::unpack_value_err(*v)?
                                .0
                                .add_to_command_line(fmt)?;
                        }
                        Ok(())
                    }
                }
            }

            fn contains_arg_attr(&self) -> bool {
                match self {
                    Impl::Item(v) => v.contains_arg_attr(),
                    Impl::List(items) => {
                        for v in *items {
                            if ValueAsCommandLineLike::unpack_value_err(*v)
                                .unwrap()
                                .0
                                .contains_arg_attr()
                            {
                                return true;
                            }
                        }
                        false
                    }
                }
            }

            fn visit_write_to_file_macros(
                &self,
                visitor: &mut dyn WriteToFileMacroVisitor,
                artifact_path_mapping: &dyn ArtifactPathMapper,
            ) -> buck2_error::Result<()> {
                match self {
                    Impl::Item(v) => v.visit_write_to_file_macros(visitor, artifact_path_mapping),
                    Impl::List(items) => {
                        for v in *items {
                            ValueAsCommandLineLike::unpack_value_err(*v)?
                                .0
                                .visit_write_to_file_macros(visitor, artifact_path_mapping)?;
                        }
                        Ok(())
                    }
                }
            }

            fn visit_artifacts(
                &self,
                visitor: &mut dyn CommandLineArtifactVisitor<'v>,
            ) -> buck2_error::Result<()> {
                match self {
                    Impl::Item(v) => v.visit_artifacts(visitor),
                    Impl::List(items) => {
                        for v in *items {
                            ValueAsCommandLineLike::unpack_value_err(*v)?
                                .0
                                .visit_artifacts(visitor)?;
                        }
                        Ok(())
                    }
                }
            }
        }

        let value = v.to_value();
        if let Some(values) = ListRef::from_value(value) {
            for v in values.content() {
                ValueAsCommandLineLike::unpack_value_err(*v)?;
            }
            Ok(Impl::List(values.content()))
        } else {
            Ok(Impl::Item(
                ValueAsCommandLineLike::unpack_value_err(value)?.0,
            ))
        }
    }
}

starlark_complex_value!(pub TransitiveSetArgsProjection);

starlark::methods_static!(
    TRANSITIVE_SET_ARGS_PROJECTION_METHODS = transitive_set_args_projection_methods
);

#[starlark_value(type = "TransitiveSetArgsProjection")]
impl<'v> StarlarkValue<'v> for TransitiveSetArgsProjection<'v>
where
    Self: ProvidesStaticType<'v>,
{
    fn get_methods() -> Option<&'static Methods> {
        Some(TRANSITIVE_SET_ARGS_PROJECTION_METHODS.methods())
    }

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}

impl<'v> CommandLineArgLike<'v> for TransitiveSetArgsProjection<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(TransitiveSetArgsProjection::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        let set = self.transitive_set.as_ref();

        if fmt.is_hashing() {
            if let Some(hash) = set.projection_hash(self.projection) {
                return fmt.push_transitive_set_projection(hash, self.projection, self.ordering);
            }
        }

        set.prefetch_children()?;
        for node in set.iter(self.ordering)?.values() {
            let node = node?;
            let projection = node
                .projections
                .get(self.projection)
                .internal_error("Invalid projection id")?;

            TransitiveSetArgsProjection::as_command_line(*projection)?.add_to_command_line(fmt)?;
        }

        Ok(())
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        let set = self.transitive_set.as_ref();

        visitor.visit_input(
            ArtifactGroup::TransitiveSetProjection(Arc::new(TransitiveSetProjectionWrapper::new(
                TransitiveSetProjectionKey {
                    key: set.key().dupe(),
                    projection: self.projection,
                },
                set.projection_path_resolution_may_require_artifact_value
                    .get(self.projection)?,
                set.projection_is_eligible_for_dedupe.get(self.projection)?,
            ))),
            InputProperties::default(),
            vec![],
        );

        Ok(())
    }

    fn contains_arg_attr(&self) -> bool {
        // TODO(cjhopman): This seems wrong, there's no enforcement that the projected values don't have arg attrs (and
        // in fact several of our common uses of in fact do have arg attrs). There doesn't seem to be any reason to
        // not allow them, either.
        false
    }

    fn visit_write_to_file_macros(
        &self,
        _visitor: &mut dyn WriteToFileMacroVisitor,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        // TODO(cjhopman): This seems wrong, there's no verification that the projected
        // values don't have write_to_file_macros in them.
        Ok(())
    }
}

#[starlark_module]
fn transitive_set_args_projection_methods(builder: &mut MethodsBuilder) {
    fn traverse<'v>(
        this: ValueTyped<'v, TransitiveSetArgsProjection<'v>>,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        Ok(heap.alloc(TransitiveSetProjectionTraversal {
            transitive_set: this.transitive_set,
            projection: this.projection,
            ordering: this.ordering,
        }))
    }

    #[starlark(attribute)]
    fn projection_name<'v>(
        this: ValueTyped<'v, TransitiveSetArgsProjection<'v>>,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str(this.projection_name()?))
    }

    #[starlark(attribute)]
    fn transitive_set<'v>(
        this: ValueTyped<'v, TransitiveSetArgsProjection<'v>>,
    ) -> starlark::Result<ValueTyped<'v, TransitiveSet<'v>>> {
        Ok(this.transitive_set)
    }
}
