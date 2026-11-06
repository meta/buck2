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
use std::hash::Hash;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::bzl::ImportPath;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::build_context::starlark_path_from_build_context;
use buck2_interpreter::paths::path::StarlarkPath;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::collections::StarlarkHasher;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::typing::Ty;
use starlark::typing::TyStarlarkValue;
use starlark::typing::TyUser;
use starlark::typing::TyUserParams;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkPagableViaPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::typing::StarlarkCallableChecked;
use starlark::values::typing::TypeInstanceId;
use starlark::values::typing::TypeMatcherFactory;
use strong_hash::StrongHash;

use crate::interpreter::rule_defs::transitive_set::TransitiveSet;
use crate::interpreter::rule_defs::transitive_set::TransitiveSetError;
use crate::interpreter::rule_defs::transitive_set::transitive_set::TransitiveSetMatcher;
use crate::interpreter::rule_defs::type_id_domain::Buck2TypeIdDomain;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum TransitiveSetDefinitionError {
    #[error("`transitive_set()` can only be used in `bzl` files")]
    TransitiveSetOnlyInBzl,
    #[error("Transitive set definition used before the module defining it finished loading")]
    UsedBeforeLoaded,
}

#[derive(
    Debug,
    Clone,
    Dupe,
    Copy,
    Trace,
    Freeze,
    PartialEq,
    Allocative,
    Pagable,
    StarlarkPagable
)]
pub enum TransitiveSetProjectionKind {
    Args,
    Json,
}

impl TransitiveSetProjectionKind {
    pub fn short_name(&self) -> &'static str {
        match self {
            TransitiveSetProjectionKind::Args => "args",
            TransitiveSetProjectionKind::Json => "json",
        }
    }

    pub fn function_name(&self) -> &'static str {
        match self {
            TransitiveSetProjectionKind::Args => "project_as_args",
            TransitiveSetProjectionKind::Json => "project_as_json",
        }
    }
}

/// A projection: `def(value) -> cmd_args | json`.
pub type ProjectionCallable<'v> =
    ValueOf<'v, StarlarkCallableChecked<'v, (Value<'static>,), Value<'static>>>;
/// A reduction: `def(children, value) -> value`.
pub type ReductionCallable<'v> = ValueOf<
    'v,
    StarlarkCallableChecked<'v, (ListType<Value<'static>>, Value<'static>), Value<'static>>,
>;

#[derive(Debug, Clone, Trace, Freeze, Allocative, StarlarkPagable)]
pub struct TransitiveSetProjectionSpec<'v> {
    #[freeze(identity)]
    pub kind: TransitiveSetProjectionKind,
    pub projection: ProjectionCallable<'v>,
}

/// A unique identity for a given [`TransitiveSetDefinition`].
#[derive(Debug, Clone, Display, Allocative, Hash, StrongHash, Pagable)]
#[display("{}", name)]
struct TransitiveSetId {
    module_id: ImportPath,
    name: String,
}

#[derive(Debug, Allocative, Pagable, StarlarkPagableViaPagable)]
pub(crate) struct TransitiveSetDefinitionExported {
    /// The name of this transitive set. This is filed in by `export_as` when it's assigned to a
    /// top-level variable. This must be set before this is used.
    id: Arc<TransitiveSetId>,
    /// Type of transitive set type.
    set_ty: Ty,
    /// Type id of transitive set type.
    pub(crate) set_type_instance_id: TypeInstanceId,
}

#[derive(Debug, ProvidesStaticType, Allocative, Trace)]
pub struct TransitiveSetDefinition<'v> {
    pub(crate) exported: std::cell::OnceCell<TransitiveSetDefinitionExported>,

    /// The module id where this `TransitiveSetDefinition` is created and assigned
    module_id: ImportPath,

    operations: TransitiveSetOperations<'v>,
}

#[derive(Debug, Clone, Trace, Freeze, Allocative, StarlarkPagable)]
pub struct TransitiveSetOperations<'v> {
    /// Callables that will project the values contained in transitive sets of this type to
    /// cmd_args or json. This can be used to include a transitive set into a command or json file.
    pub(crate) projections: BuckMutInsertionMap<String, TransitiveSetProjectionSpec<'v>>,

    /// Callables that will reduce the values contained in transitive sets to a single value per
    /// node. This can be used to e.g. aggregate flags throughout a transitive set;
    pub(crate) reductions: BuckMutInsertionMap<String, ReductionCallable<'v>>,
}

impl<'v> TransitiveSetOperations<'v> {
    pub fn valid_projections(&self, kind: TransitiveSetProjectionKind) -> Vec<String> {
        self.projections
            .iter()
            .filter_map(|(k, spec)| {
                if kind == spec.kind {
                    Some(k.to_owned())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }

    pub fn get_index_of_projection(
        &self,
        kind: TransitiveSetProjectionKind,
        proj: &str,
    ) -> buck2_error::Result<usize> {
        let index = match self.projections.get_index_of(proj) {
            Some(index) => index,
            None => {
                return Err(TransitiveSetError::ProjectionDoesNotExist {
                    projection: proj.to_owned(),
                    valid_projections: self.valid_projections(kind),
                }
                .into());
            }
        };

        let (_, spec) = self.projections.get_index(index).unwrap();
        if spec.kind != kind {
            return Err(TransitiveSetError::ProjectionKindMismatch {
                projection: proj.to_owned(),
                expected_kind: kind,
                actual_kind: spec.kind,
            }
            .into());
        }

        Ok(index)
    }
}

impl<'v> Display for TransitiveSetDefinition<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.exported.get() {
            Some(exported) => {
                write!(f, "{}", exported.id)
            }
            None => write!(f, "unnamed transitive set"),
        }
    }
}

impl<'v> Serialize for TransitiveSetDefinition<'v> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&format!("{self}"))
    }
}

impl<'v> TransitiveSetDefinition<'v> {
    fn new(module_id: ImportPath, operations: TransitiveSetOperations<'v>) -> Self {
        Self {
            exported: std::cell::OnceCell::new(),
            module_id,
            operations,
        }
    }

    pub fn has_id(&self) -> bool {
        self.exported.get().is_some()
    }
}

impl<'v> AllocValue<'v> for TransitiveSetDefinition<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

/// A transitive set definition in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum TransitiveSetDefinitionView<'v> {
    Frozen(ValueTyped<'v, FrozenTransitiveSetDefinition<'v>>),
    Unfrozen(ValueTyped<'v, TransitiveSetDefinition<'v>>),
}

impl<'v> TransitiveSetDefinitionView<'v> {
    /// The definition once its module is loaded, which is what creating a set requires: the set
    /// keeps the definition, and a definition is only unfrozen while its module loads, when no
    /// analysis runs.
    pub fn frozen(
        self,
    ) -> buck2_error::Result<FrozenValueTyped<'v, FrozenTransitiveSetDefinition<'v>>> {
        match self {
            TransitiveSetDefinitionView::Frozen(this) => FrozenValueTyped::new_err(this.to_value())
                .internal_error("the frozen representation is only allocated on a frozen heap"),
            TransitiveSetDefinitionView::Unfrozen(_) => {
                Err(TransitiveSetDefinitionError::UsedBeforeLoaded.into())
            }
        }
    }

    /// What `export_as` establishes; always present once frozen.
    fn exported(&self) -> Option<&'v TransitiveSetDefinitionExported> {
        match self {
            TransitiveSetDefinitionView::Frozen(this) => Some(&this.as_ref().exported),
            TransitiveSetDefinitionView::Unfrozen(this) => this.as_ref().exported.get(),
        }
    }
}

#[starlark_value(type = "TransitiveSetDefinition")]
impl<'v> StarlarkType<'v> for TransitiveSetDefinitionView<'v> {
    fn export_as(
        &self,
        variable_name: &str,
        _: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<()> {
        let TransitiveSetDefinitionView::Unfrozen(this) = self else {
            return Ok(());
        };
        // First export wins
        this.exported.get_or_try_init(|| {
            let id = Arc::new(TransitiveSetId {
                module_id: this.module_id.clone(),
                name: variable_name.to_owned(),
            });
            let set_type_instance_id =
                TypeInstanceId::from_identity(Buck2TypeIdDomain::TransitiveSet, &*id);
            let set_ty = Ty::custom(TyUser::new(
                variable_name.to_owned(),
                TyStarlarkValue::new::<TransitiveSet>(),
                set_type_instance_id,
                TyUserParams {
                    matcher: Some(TypeMatcherFactory::new(TransitiveSetMatcher {
                        type_instance_id: set_type_instance_id,
                    })),

                    ..TyUserParams::default()
                },
            )?);
            buck2_error::Ok(TransitiveSetDefinitionExported {
                id,
                set_ty,
                set_type_instance_id,
            })
        })?;
        Ok(())
    }

    fn dir_attr(&self) -> Vec<String> {
        vec!["type".to_owned()]
    }

    fn has_attr(&self, attribute: &str, _heap: Heap<'v>) -> bool {
        attribute == "type"
    }

    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        if attribute == "type" {
            let typ = self
                .exported()
                .map_or("TransitiveSetDefinition", |exported| {
                    exported.id.name.as_str()
                });
            Some(heap.alloc(typ))
        } else {
            None
        }
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        let exported = self
            .exported()
            .internal_error("cannot hash a transitive_set_definition without id")?;
        exported.id.hash(hasher);
        Ok(())
    }

    fn eval_type(&self) -> Option<Ty> {
        self.exported().map(|exported| exported.set_ty.dupe())
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for TransitiveSetDefinition<'v> {
    type Type = TransitiveSetDefinitionView<'v>;
}

#[derive(Display, ProvidesStaticType, Allocative, StarlarkPagable)]
#[display("{}", exported.id)]
pub struct FrozenTransitiveSetDefinition<'v> {
    pub(crate) exported: TransitiveSetDefinitionExported,

    operations: TransitiveSetOperations<'v>,
}

impl<'v> fmt::Debug for FrozenTransitiveSetDefinition<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TransitiveSetDefinition({} declared in {})",
            self.exported.id.name, self.exported.id.module_id
        )
    }
}

impl<'v> Serialize for FrozenTransitiveSetDefinition<'v> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&format!("{self}"))
    }
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenTransitiveSetDefinition<'v> {
    type Type = TransitiveSetDefinitionView<'v>;
}

impl<'v> Freeze<'v> for TransitiveSetDefinition<'v> {
    type Frozen<'fv> = FrozenTransitiveSetDefinition<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let Self {
            exported,
            module_id: _,
            operations,
        } = self;

        let exported = match exported.into_inner() {
            Some(x) => x,
            None => {
                // Unfortunately we have no name or location for the definition at this point.
                return Err(FreezeError::new(
                    TransitiveSetError::TransitiveSetNotAssigned.to_string(),
                ));
            }
        };

        let operations = operations.freeze(freezer)?;

        Ok(FrozenTransitiveSetDefinition {
            exported,
            operations,
        })
    }
}

pub trait TransitiveSetDefinitionLike<'v> {
    fn has_id(&self) -> bool;

    fn as_debug(&self) -> &dyn fmt::Debug;

    fn operations(&self) -> &TransitiveSetOperations<'v>;
}

impl<'v> TransitiveSetDefinitionLike<'v> for TransitiveSetDefinition<'v> {
    fn has_id(&self) -> bool {
        Self::has_id(self)
    }

    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }

    fn operations(&self) -> &TransitiveSetOperations<'v> {
        &self.operations
    }
}

impl<'v> TransitiveSetDefinitionLike<'v> for FrozenTransitiveSetDefinition<'v> {
    fn has_id(&self) -> bool {
        true
    }

    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }

    fn operations(&self) -> &TransitiveSetOperations<'v> {
        &self.operations
    }
}

#[starlark_module]
pub fn register_transitive_set(builder: &mut GlobalsBuilder) {
    fn transitive_set<'v>(
        #[starlark(require = named)] args_projections: Option<
            ValueOf<'v, DictType<String, ProjectionCallable<'v>>>,
        >,
        #[starlark(require = named)] json_projections: Option<
            ValueOf<'v, DictType<String, ProjectionCallable<'v>>>,
        >,
        #[starlark(require = named)] reductions: Option<
            ValueOf<'v, DictType<String, ReductionCallable<'v>>>,
        >,
        eval: &mut Evaluator,
    ) -> starlark::Result<TransitiveSetDefinition<'v>> {
        let projections: BuckMutInsertionMap<_, _> = args_projections
            .into_iter()
            .flat_map(|v| v.try_items())
            .map(|entry| {
                let (k, v) = entry?;
                Ok::<_, starlark::Error>((
                    k,
                    TransitiveSetProjectionSpec {
                        kind: TransitiveSetProjectionKind::Args,
                        projection: v,
                    },
                ))
            })
            .chain(
                json_projections
                    .into_iter()
                    .flat_map(|v| v.try_items())
                    .map(|entry| {
                        let (k, v) = entry?;
                        Ok::<_, starlark::Error>((
                            k,
                            TransitiveSetProjectionSpec {
                                kind: TransitiveSetProjectionKind::Json,
                                projection: v,
                            },
                        ))
                    }),
            )
            .collect::<starlark::Result<_>>()?;

        let reductions = match reductions {
            Some(reductions) => reductions.try_items().collect::<starlark::Result<_>>()?,
            None => BuckMutInsertionMap::new(),
        };

        let starlark_path: StarlarkPath = starlark_path_from_build_context(eval)?;
        Ok(TransitiveSetDefinition::new(
            match starlark_path {
                StarlarkPath::LoadFile(import_path) => import_path.clone(),
                _ => {
                    return Err(buck2_error::Error::from(
                        TransitiveSetDefinitionError::TransitiveSetOnlyInBzl,
                    )
                    .into());
                }
            },
            TransitiveSetOperations {
                projections,
                reductions,
            },
        ))
    }
}
