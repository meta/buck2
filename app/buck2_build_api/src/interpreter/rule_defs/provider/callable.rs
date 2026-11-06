/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::provider::id::ProviderId;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_interpreter::build_context::starlark_path_from_build_context;
use buck2_interpreter::types::provider::callable::ProviderCallableLike;
use dupe::Dupe;
use either::Either;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::any::ProvidesStaticType;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocProperty;
use starlark::docs::DocString;
use starlark::docs::DocStringKind;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Arguments;
use starlark::eval::Evaluator;
use starlark::eval::ParametersSpec;
use starlark::eval::ParametersSpecParam;
use starlark::eval::param_specs;
use starlark::type_matcher;
use starlark::typing::Ty;
use starlark::typing::TyCallable;
use starlark::typing::TyStarlarkValue;
use starlark::values::AllocFrozenValue;
use starlark::values::AllocValue;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenHeap;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Tracer;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::any_complex::StarlarkAnyComplex;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::list::ListRef;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::starlark_value;
use starlark::values::typing::TypeCompiled;
use starlark::values::typing::TypeInstanceId;
use starlark::values::typing::TypeMatcher;
use starlark::values::typing::TypeMatcherDyn;
use starlark::values::typing::TypeMatcherFactory;

use crate::interpreter::rule_defs::provider::doc::ProviderMembersSource;
use crate::interpreter::rule_defs::provider::doc::provider_callable_documentation;
use crate::interpreter::rule_defs::provider::ty::abstract_provider::AbstractProvider;
use crate::interpreter::rule_defs::provider::ty::provider::ty_provider;
use crate::interpreter::rule_defs::provider::ty::provider_callable::ty_provider_callable;
use crate::interpreter::rule_defs::provider::user::UserProvider;
use crate::interpreter::rule_defs::provider::user::user_provider_creator;
use crate::interpreter::rule_defs::type_id_domain::Buck2TypeIdDomain;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ProviderCallableError {
    #[error(
        "The result of `provider()` must be assigned to a top-level variable before it can be called"
    )]
    NotBound,
    #[error(
        "Provider type must be assigned to a variable, e.g. `ProviderInfo = provider(fields = {0:?})`"
    )]
    ProviderNotAssigned(BuckMutInsertionSet<String>),
    #[error("non-unique field names: [{}]", .0.iter().map(|s| format!("`{s}`")).join(", "))]
    NonUniqueFields(Vec<String>),
    #[error("Field default value can be either frozen value or an empty list or dict")]
    InvalidDefaultValue,
    #[error("Default value `{0}` (type `{1}`) does not match field type `{2}`")]
    InvalidDefaultValueType(String, &'static str, Ty),
}

fn create_callable_function_signature<'v>(
    function_name: &str,
    fields: &BuckMutInsertionMap<String, UserProviderField<'v>>,
    ret_ty: Ty,
) -> buck2_error::Result<(ParametersSpec<Value<'v>>, TyCallable)> {
    let (parameters_spec, param_spec) = param_specs(
        function_name,
        [],
        [],
        None,
        fields.iter().map(|(name, field)| {
            (
                name.as_str(),
                match field.default {
                    None => ParametersSpecParam::Required,
                    Some(default) => ParametersSpecParam::Defaulted(default),
                },
                field.ty.as_ty().dupe(),
            )
        }),
        None,
    )
    .internal_error("Must have created correct signature")?;

    Ok((parameters_spec, TyCallable::new(param_spec, ret_ty)))
}

/// What provider instances know about their callable: the fields, by name and type. The
/// defaults live in the callable's signature.
///
/// One copy per provider type, shared by the callable and every instance: allocated in the
/// module's frozen heap as a `StarlarkAnyComplex` (see [`UserProviderCallableDataValue`]).
#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
pub(crate) struct UserProviderCallableData<'v> {
    #[starlark_pagable(pagable)]
    pub(crate) provider_id: Arc<ProviderId>,
    /// Type id of provider callable instance.
    pub(crate) ty_provider_type_instance_id: TypeInstanceId,
    pub(crate) fields: BuckMutInsertionMap<String, TypeCompiled<'v>>,
}

starlark::register_starlark_any_complex!(frozen UserProviderCallableData<'_>);

/// A [`UserProviderCallableData`] as the value it is allocated as, at the brand of the heap that
/// holds it.
pub(crate) type UserProviderCallableDataValue<'v> =
    FrozenValueTyped<'v, StarlarkAnyComplex<UserProviderCallableData<'v>>>;

/// Initialized after the name is assigned to the provider.
#[derive(Debug, Trace, Allocative, StarlarkPagable)]
struct UserProviderCallableNamed<'v> {
    /// The name of this provider, filled in by `export_as()`. This must be set before this
    /// object can be called and Providers created.
    #[starlark_pagable(pagable)]
    id: Arc<ProviderId>,
    signature: ParametersSpec<Value<'v>>,
    /// This field is shared with provider instances.
    data: UserProviderCallableDataValue<'v>,
    /// Type of provider instance.
    #[starlark_pagable(pagable)]
    ty_provider: Ty,
    /// Type of provider callable.
    #[starlark_pagable(pagable)]
    ty_callable: Ty,
}

impl<'v> UserProviderCallableNamed<'v> {
    fn invoke(
        &self,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        self.signature.parser(args, eval, |parser, eval| {
            user_provider_creator(self.data, eval, parser).map_err(Into::into)
        })
    }
}

impl<'v> Freeze<'v> for UserProviderCallableNamed<'v> {
    type Frozen<'fv> = UserProviderCallableNamed<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let UserProviderCallableNamed {
            id,
            signature,
            data,
            ty_provider,
            ty_callable,
        } = self;
        Ok(UserProviderCallableNamed {
            id,
            signature: signature.freeze(freezer)?,
            data: data.freeze(freezer)?,
            ty_provider,
            ty_callable,
        })
    }
}

#[derive(
    Debug,
    Trace,
    Freeze,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    Clone,
    Dupe,
    StarlarkPagable
)]
pub(crate) struct UserProviderField<'v> {
    /// Field type.
    pub(crate) ty: TypeCompiled<'v>,
    /// Default value. If `None`, the field is required. Always immutable, so that instances can
    /// share it.
    pub(crate) default: Option<Value<'v>>,
}

starlark_complex_value!(pub(crate) UserProviderField);

impl<'v> Display for UserProviderField<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "ProviderField({}, ", self.ty)?;
        if let Some(default) = &self.default {
            write!(f, "default = {default}")?;
        } else {
            write!(f, "required")?;
        }
        write!(f, ")")
    }
}

impl<'v> UserProviderField<'v> {
    pub(crate) fn default() -> UserProviderField<'v> {
        UserProviderField {
            ty: TypeCompiled::any(),
            default: Some(Value::new_none()),
        }
    }
}

#[starlark_value(type = "ProviderField")]
impl<'v> StarlarkValue<'v> for UserProviderField<'v> {}

/// The result of calling `provider()`. This is a callable that accepts the fields
/// provided in the `provider()` call, and generates a Starlark `UserProvider` object.
///
/// This object must be assigned to a variable at the top level of the module before it may be invoked
///
/// Field values default to `None`
#[derive(
    Debug,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    starlark::StarlarkPagable
)]
pub struct UserProviderCallable<'v> {
    /// The path where this `ProviderCallable` is created and assigned
    #[starlark_pagable(pagable)]
    path: CellPath,
    /// The docstring for this provider
    #[starlark_pagable(pagable)]
    docs: Option<DocString>,
    /// The names of the fields used in `callable`
    fields: BuckMutInsertionMap<String, UserProviderField<'v>>,
    /// Field is initialized after the provider is assigned to a variable.
    callable: OnceCell<UserProviderCallableNamed<'v>>,
}

fn user_provider_callable_display(
    id: Option<&Arc<ProviderId>>,
    fields: &BuckMutInsertionMap<String, UserProviderField<'_>>,
    f: &mut Formatter,
) -> fmt::Result {
    write!(f, "provider")?;
    if let Some(id) = id {
        write!(f, "[{}]", id.name)?;
    }
    write!(f, "(fields={{")?;
    for (i, (name, ty)) in fields.iter().enumerate() {
        if i != 0 {
            write!(f, ", ")?;
        }
        write!(f, "\"{}\": provider_field({}", name, ty.ty)?;
        if let Some(default) = ty.default {
            write!(f, ", default={default}")?;
        }
        write!(f, ")")?;
    }
    write!(f, "}})")?;
    Ok(())
}

impl<'v> Display for UserProviderCallable<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        user_provider_callable_display(self.callable.get().map(|x| &x.id), &self.fields, f)
    }
}

unsafe impl<'v> Trace<'v> for UserProviderCallable<'v> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        for field in self.fields.values_mut() {
            field.trace(tracer);
        }
        self.callable.trace(tracer);
    }
}

impl<'v> UserProviderCallable<'v> {
    fn new(
        path: CellPath,
        docs: Option<DocString>,
        fields: BuckMutInsertionMap<String, UserProviderField<'v>>,
    ) -> Self {
        Self {
            callable: OnceCell::new(),
            path,
            docs,
            fields,
        }
    }
}

impl<'v> ProviderCallableLike for UserProviderCallable<'v> {
    fn id(&self) -> buck2_error::Result<&Arc<ProviderId>> {
        self.callable
            .get()
            .map(|x| &x.id)
            .ok_or(ProviderCallableError::NotBound.into())
    }
}

impl<'v> AllocValue<'v> for UserProviderCallable<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

impl<'v> Freeze<'v> for UserProviderCallable<'v> {
    type Frozen<'fv> = FrozenUserProviderCallable<'fv>;
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let callable = self.callable.into_inner();
        let callable = match callable {
            Some(x) => x,
            None => {
                // Unfortunately we have no name or location for the provider at this point,
                // so reproduce the fields so that the provider can be identified.
                return Err(FreezeError::new(
                    ProviderCallableError::ProviderNotAssigned(
                        self.fields.into_iter().map(|(name, _)| name).collect(),
                    )
                    .to_string(),
                ));
            }
        };

        let mut fields = BuckMutInsertionMap::with_capacity(self.fields.len());
        for (name, field) in self.fields {
            fields.insert(name, field.freeze(freezer)?);
        }

        Ok(FrozenUserProviderCallable::new(
            self.docs,
            fields,
            callable.freeze(freezer)?,
        ))
    }
}

#[derive(Debug, Clone, Allocative, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
struct UserProviderMatcher {
    type_instance_id: TypeInstanceId,
}

#[type_matcher]
impl TypeMatcher for UserProviderMatcher {
    fn matches(&self, value: Value) -> bool {
        match UserProvider::from_value(value) {
            Some(x) => {
                // TODO(nga): this is a bit suboptimal:
                //   instead we could compare just a pointer to the callable.
                x.callable_data().ty_provider_type_instance_id == self.type_instance_id
            }
            None => false,
        }
    }
}

/// A provider callable in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum UserProviderCallableView<'v> {
    Frozen(ValueTyped<'v, FrozenUserProviderCallable<'v>>),
    Unfrozen(ValueTyped<'v, UserProviderCallable<'v>>),
}

impl<'v> UserProviderCallableView<'v> {
    fn docs(&self) -> &'v Option<DocString> {
        match self {
            UserProviderCallableView::Frozen(this) => &this.as_ref().docs,
            UserProviderCallableView::Unfrozen(this) => &this.as_ref().docs,
        }
    }

    fn fields(&self) -> &'v BuckMutInsertionMap<String, UserProviderField<'v>> {
        match self {
            UserProviderCallableView::Frozen(this) => &this.as_ref().fields,
            UserProviderCallableView::Unfrozen(this) => &this.as_ref().fields,
        }
    }

    /// What `export_as` establishes; always present once frozen.
    fn callable(&self) -> Option<&'v UserProviderCallableNamed<'v>> {
        match self {
            UserProviderCallableView::Frozen(this) => Some(&this.as_ref().callable),
            UserProviderCallableView::Unfrozen(this) => this.as_ref().callable.get(),
        }
    }
}

#[starlark_value(type = "ProviderCallable")]
impl<'v> StarlarkType<'v> for UserProviderCallableView<'v> {
    fn export_as(
        &self,
        variable_name: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<()> {
        let UserProviderCallableView::Unfrozen(this) = self else {
            return Ok(());
        };
        // First export wins
        this.callable.get_or_try_init(|| {
            let provider_id = Arc::new(ProviderId {
                path: Some(this.path),
                name: variable_name.to_owned(),
            });
            let ty_provider_type_instance_id =
                TypeInstanceId::from_identity(Buck2TypeIdDomain::UserProvider, &*provider_id);
            let ty_provider = ty_provider(
                &provider_id.name,
                ty_provider_type_instance_id,
                TyStarlarkValue::new::<UserProvider>(),
                Some(TypeMatcherFactory::new(UserProviderMatcher {
                    type_instance_id: ty_provider_type_instance_id,
                })),
                this.fields
                    .iter()
                    .map(|(name, field)| (name.to_owned(), field.ty.as_ty().dupe()))
                    .collect(),
            )?;
            let (signature, creator_func) = create_callable_function_signature(
                &provider_id.name,
                &this.fields,
                ty_provider.clone(),
            )?;
            // The frozen representation is the one with a registered typing vtable; both
            // representations' vtables are the type's.
            let ty_callable = ty_provider_callable::<FrozenUserProviderCallable>(creator_func)?;
            buck2_error::Ok(UserProviderCallableNamed {
                id: provider_id.dupe(),
                signature,
                data: eval.frozen_heap(|fh, edge| {
                    let data =
                        fh.alloc_simple_typed(StarlarkAnyComplex::new(UserProviderCallableData {
                            provider_id,
                            fields: this
                                .fields
                                .iter()
                                .map(|(name, field)| (name.clone(), field.ty.to_frozen(fh)))
                                .collect(),
                            ty_provider_type_instance_id,
                        }));
                    let data =
                        FrozenValueTyped::from_typed(data).expect("allocated in a frozen heap");
                    edge.rebrand(data)
                }),
                ty_provider,
                ty_callable,
            })
        })?;
        Ok(())
    }

    fn invoke(
        &self,
        _me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        match self.callable() {
            Some(callable) => callable.invoke(args, eval),
            None => Err(buck2_error::Error::from(ProviderCallableError::NotBound).into()),
        }
    }

    fn eval_type(&self) -> Option<Ty> {
        self.callable().map(|named| named.ty_provider.dupe())
    }

    fn documentation(&self) -> DocItem {
        let fields = self.fields();
        let return_types = vec![Ty::any(); fields.len()];
        let Some(callable) = self.callable() else {
            // This shouldn't really happen, we mostly don't even ask for documentation on
            // non-frozen things
            return DocItem::Member(DocMember::Property(DocProperty {
                docs: None,
                typ: Ty::any(),
            }));
        };
        let field_names: Vec<_> = fields.keys().map(|x| x.as_str()).collect();
        provider_callable_documentation(
            None,
            ProviderMembersSource::FromFields {
                fields: &field_names,
                // TODO(nga): types.
                field_docs: &vec![None; fields.len()],
                field_types: &return_types,
            },
            callable.ty_callable.dupe(),
            self.docs(),
        )
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        self.callable().map(|named| named.ty_callable.dupe())
    }
}

#[starlark_value(skip_vtable)]
impl<'v> StarlarkValue<'v> for UserProviderCallable<'v> {
    type Type = UserProviderCallableView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn ProviderCallableLike>(self);
    }
}

#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagable)]
pub struct FrozenUserProviderCallable<'v> {
    /// The docstring for this provider
    #[starlark_pagable(pagable)]
    docs: Option<DocString>,
    /// The names of the fields used in `callable`
    fields: BuckMutInsertionMap<String, UserProviderField<'v>>,
    /// The actual callable that creates instances of `UserProvider`
    callable: UserProviderCallableNamed<'v>,
}

impl<'fv> AllocFrozenValue<'fv> for FrozenUserProviderCallable<'fv> {
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        heap.alloc_simple_typed(self).to_value()
    }
}

impl<'v> Display for FrozenUserProviderCallable<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        user_provider_callable_display(Some(&self.callable.id), &self.fields, f)
    }
}

impl<'v> FrozenUserProviderCallable<'v> {
    fn new(
        docs: Option<DocString>,
        fields: BuckMutInsertionMap<String, UserProviderField<'v>>,
        callable: UserProviderCallableNamed<'v>,
    ) -> Self {
        Self {
            docs,
            fields,
            callable,
        }
    }
}

impl<'v> ProviderCallableLike for FrozenUserProviderCallable<'v> {
    fn id(&self) -> buck2_error::Result<&Arc<ProviderId>> {
        Ok(&self.callable.id)
    }
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenUserProviderCallable<'v> {
    type Type = UserProviderCallableView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn ProviderCallableLike>(self);
    }
}

fn provider_field_parse_type<'v>(
    ty: Value<'v>,
    eval: &mut Evaluator<'v, '_, '_>,
) -> buck2_error::Result<TypeCompiled<'v>> {
    TypeCompiled::new(ty, eval.heap())
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Interpreter))
}

#[starlark_module]
#[starlark_types(AbstractProvider as Provider no_docs)]
pub fn register_provider(builder: &mut GlobalsBuilder) {
    /// Create a field definition object which can be passed to `provider` type constructor.
    fn provider_field<'v>(
        #[starlark(require=pos)] ty: Value<'v>,
        #[starlark(require=named)] default: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<UserProviderField<'v>> {
        let ty = provider_field_parse_type(ty, eval)?;
        let default = match default {
            None => None,
            Some(x) => {
                // The default is shared by every instance, so it must be immutable: frozen, or
                // the immutable empty list or dict.
                if x.is_frozen() {
                    Some(x)
                } else if ListRef::from_value(x).is_some_and(|x| x.is_empty()) {
                    Some(Value::new_empty_list())
                } else if DictRef::from_value(x).is_some_and(|x| x.is_empty()) {
                    Some(Value::new_empty_dict())
                } else {
                    return Err(buck2_error::Error::from(
                        ProviderCallableError::InvalidDefaultValue,
                    )
                    .into());
                }
            }
        };
        if let Some(default) = default {
            if !ty.matches(default) {
                return Err(buck2_error::Error::from(
                    ProviderCallableError::InvalidDefaultValueType(
                        default.to_string(),
                        default.get_type(),
                        ty.as_ty().dupe(),
                    ),
                )
                .into());
            }
        }
        Ok(UserProviderField { ty, default })
    }

    /// Create a `"provider"` type that can be returned from `rule` implementations.
    /// Used to pass information from a rule to the things that depend on it.
    /// Typically named with an `Info` suffix.
    ///
    /// ```python
    /// GroovyLibraryInfo(fields = [
    ///     "objects",  # a list of artifacts
    ///     "options",  # a string containing compiler options
    /// ])
    /// ```
    ///
    /// Given a dependency you can obtain the provider with `my_dep[GroovyLibraryInfo]`
    /// which returns either `None` or a value of type `GroovyLibraryInfo`.
    ///
    /// For providers that accumulate upwards a transitive set is often a good choice.
    fn provider<'v>(
        #[starlark(require=named, default = "")] doc: &str,
        #[starlark(require=named)] fields: Either<
            ValueOf<'v, ListOrTupleType<String>>,
            ValueOf<'v, DictType<String, Value<'v>>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<UserProviderCallable<'v>> {
        let docstring = DocString::from_docstring(DocStringKind::Starlark, doc);
        let path = starlark_path_from_build_context(eval)?.path();

        let fields = match fields {
            Either::Left(fields) => {
                let fields = fields.try_to_vec()?;
                let new_fields: BuckMutInsertionMap<String, UserProviderField<'v>> = fields
                    .iter()
                    .map(|name| (name.clone(), UserProviderField::default()))
                    .collect();
                if new_fields.len() != fields.len() {
                    return Err(
                        buck2_error::Error::from(ProviderCallableError::NonUniqueFields(fields))
                            .into(),
                    );
                }
                new_fields
            }
            Either::Right(fields) => {
                let mut new_fields = BuckMutInsertionMap::with_capacity(fields.dict().len());
                for entry in fields.try_items() {
                    let (name, field) = entry?;
                    if let Some(field) = field.downcast_ref::<UserProviderField>() {
                        new_fields.insert(name, field.dupe());
                    } else {
                        let ty = provider_field_parse_type(field, eval)
                            .with_buck_error_context(|| format!("Field `{name}` type `{field}` is not created with `provider_field`, and cannot be evaluated as a type"))?;
                        new_fields.insert(name, UserProviderField { ty, default: None });
                    }
                }
                new_fields
            }
        };
        Ok(UserProviderCallable::new(
            path.into_owned(),
            docstring,
            fields,
        ))
    }
}
