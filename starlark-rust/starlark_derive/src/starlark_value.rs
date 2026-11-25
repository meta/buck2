/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::collections::HashMap;

use quote::ToTokens;
use quote::quote;
use syn::spanned::Spanned;

use crate::membership::memberships_impl;
use crate::membership::take_memberships;
use crate::typed_behaviour::Typing;
use crate::typed_behaviour::always_hashes_impl;
use crate::typed_behaviour::derive_from_typed;
use crate::v_lifetime::find_v_lifetime;
use crate::vtable::is_self_get;
use crate::vtable::vtable_has_field_name;

pub(crate) fn derive_starlark_value(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr = syn::parse_macro_input!(attr as StarlarkValueAttrs);
    let input = syn::parse_macro_input!(input as syn::ItemImpl);
    match derive_starlark_value_impl(attr, input) {
        Ok(r#gen) => r#gen.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

struct StarlarkValueAttrs {
    /// The `type()` string. Required unless the impl is a representation that names its
    /// `Type`, whose string it is then.
    typ: Option<syn::Expr>,
    vtable: VtableMode,
    alloc: AllocMode,
    /// `type_expr = by_hand`: the type writes its own `StarlarkTypeExpr`,
    /// `StarlarkTypeExprStable` and `UnionPayload` impls. A container does: its Rust type is
    /// generic over the item type and is the `StarlarkType` at one instantiation only.
    type_expr_by_hand: bool,
    /// `methods = path`: the `#[starlark_module]` function whose members are the type's
    /// methods and attributes. `methods = [path, path]` for a type that has the members of two
    /// modules.
    methods: Vec<syn::Path>,
}

/// How the representation is allocated, which decides the `AllocRepresentation` and
/// `AllocFrozenRepresentation` impls written next to the type; see
/// `ImplStarlarkValue::alloc_impls`.
#[derive(Copy, Clone, PartialEq, Eq)]
enum AllocMode {
    /// Decided by the shape of `Self`'s generics and the vtable flags.
    Default,
    /// `alloc = simple`: a `'static` representation, on either heap.
    Simple,
    /// `alloc = complex`: traced and frozen by its `Freeze` impl; also on the frozen heap when
    /// `frozen_vtable` says the type is its own frozen form.
    Complex,
    /// `no_freeze`: traced, never frozen, and only on the unfrozen heap.
    ComplexNoFreeze,
    /// `alloc = frozen`: a frozen form, only on the frozen heap.
    Frozen,
    /// `alloc = by_hand`: the representation writes its own impls, as an inline one must.
    ByHand,
}

/// Which vtable registrations are emitted next to the impl; see
/// `ImplStarlarkValue::vtable_registration`. At most one of the flags may be given.
#[derive(Copy, Clone, PartialEq, Eq)]
enum VtableMode {
    /// Decided by the shape of `Self`'s generics.
    Default,
    /// `skip_vtable`: register nothing. For types whose frozen representation handles
    /// serialization.
    Skip,
    /// `no_freeze`: register only the typing-side vtable (`TyStarlarkValueVTable`); skip the
    /// pagable `AValue` vtable that would require `T: StarlarkPagable + Sync`. The type is
    /// never frozen, and is allocated so.
    TyOnly,
    /// `frozen_vtable`: a lifetime-parameterized `T<'v>` is its own frozen form, so besides the
    /// typing-side vtable register the pagable `AValue` vtable for `T<'static>`, which is what
    /// makes every `T<'v>` `VtableRegistered`. Requires `T<'static>: StarlarkPagable + Sync`.
    Frozen,
}

impl StarlarkValueAttrs {
    fn set_alloc_mode(&mut self, name: &syn::Ident, mode: AllocMode) -> syn::Result<()> {
        if self.alloc != AllocMode::Default {
            return Err(syn::Error::new_spanned(
                name,
                "at most one of `no_freeze` and `alloc = ...` may be given",
            ));
        }
        self.alloc = mode;
        Ok(())
    }

    fn set_vtable_mode(&mut self, name: &syn::Ident, mode: VtableMode) -> syn::Result<()> {
        if self.vtable != VtableMode::Default {
            return Err(syn::Error::new_spanned(
                name,
                "at most one of `skip_vtable`, `no_freeze`, `frozen_vtable` may be given",
            ));
        }
        self.vtable = mode;
        Ok(())
    }
}

impl syn::parse::Parse for StarlarkValueAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let typ = if input.peek(syn::Token![type]) {
            input.parse::<syn::Token![type]>()?;
            input.parse::<syn::Token![=]>()?;
            Some(input.parse::<syn::Expr>()?)
        } else {
            None
        };
        let mut attrs = StarlarkValueAttrs {
            typ,
            vtable: VtableMode::Default,
            alloc: AllocMode::Default,
            type_expr_by_hand: false,
            methods: Vec::new(),
        };

        loop {
            if input.is_empty() {
                break;
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
                if input.is_empty() {
                    // Allow trailing comma.
                    break;
                }
            }
            let name = input.parse::<syn::Ident>()?;
            if name == "skip_vtable" {
                attrs.set_vtable_mode(&name, VtableMode::Skip)?;
            } else if name == "no_freeze" {
                attrs.set_vtable_mode(&name, VtableMode::TyOnly)?;
                attrs.set_alloc_mode(&name, AllocMode::ComplexNoFreeze)?;
            } else if name == "frozen_vtable" {
                attrs.set_vtable_mode(&name, VtableMode::Frozen)?;
            } else if name == "methods" {
                input.parse::<syn::Token![=]>()?;
                if !attrs.methods.is_empty() {
                    return Err(syn::Error::new_spanned(name, "`methods` is given twice"));
                }
                if input.peek(syn::token::Bracket) {
                    let list;
                    syn::bracketed!(list in input);
                    let modules = list.parse_terminated(syn::Path::parse, syn::Token![,])?;
                    if modules.len() != 2 {
                        return Err(syn::Error::new_spanned(
                            name,
                            "`methods = [..]` names two modules; a single one is named without brackets",
                        ));
                    }
                    attrs.methods = modules.into_iter().collect();
                } else {
                    attrs.methods = vec![input.parse::<syn::Path>()?];
                }
            } else if name == "alloc" {
                input.parse::<syn::Token![=]>()?;
                let kind = input.parse::<syn::Ident>()?;
                let mode = if kind == "simple" {
                    AllocMode::Simple
                } else if kind == "complex" {
                    AllocMode::Complex
                } else if kind == "frozen" {
                    AllocMode::Frozen
                } else if kind == "by_hand" {
                    AllocMode::ByHand
                } else {
                    return Err(syn::Error::new_spanned(
                        kind,
                        "`alloc` is one of `simple`, `complex`, `frozen`, `by_hand`; a representation that cannot be frozen says `no_freeze`",
                    ));
                };
                attrs.set_alloc_mode(&name, mode)?;
            } else if name == "type_expr" {
                input.parse::<syn::Token![=]>()?;
                let kind = input.parse::<syn::Ident>()?;
                if kind != "by_hand" {
                    return Err(syn::Error::new_spanned(kind, "`type_expr` is `by_hand`"));
                }
                attrs.type_expr_by_hand = true;
            } else {
                return Err(syn::Error::new_spanned(
                    name,
                    "unknown attribute, allowed attributes are `skip_vtable`, `no_freeze`, `frozen_vtable`, `alloc = ...`, `methods = ...`, `type_expr = by_hand`",
                ));
            }
        }

        Ok(attrs)
    }
}

/// Which trait the block under the attribute is an impl of.
#[derive(Copy, Clone, PartialEq, Eq)]
enum ImplOf {
    /// `impl StarlarkValue for X`: `X` is a representation, and unless it names a `Type`, its
    /// type too, with the type's behaviour written in this block.
    Value,
    /// `impl StarlarkType for X`: `X` is a type whose representations are elsewhere.
    Type,
}

/// What the methods of a block that are written with typed signatures say to the typechecker, by
/// the name of the method.
type TypedMethods = HashMap<String, Typing>;

struct ImplStarlarkValue {
    input: syn::ItemImpl,
    impl_of: ImplOf,
    lifetime_param: syn::Lifetime,
    attrs: StarlarkValueAttrs,
}

/// The `type Get` or the `fn get` of a block: the items of the type as a type expression.
fn is_get_item(item: &syn::ImplItem) -> bool {
    match item {
        syn::ImplItem::Type(t) => t.ident == "Get",
        syn::ImplItem::Fn(f) => f.sig.ident == "get",
        _ => false,
    }
}

/// An item of an `impl StarlarkValue` block that stays on the representation. Everything else
/// is the type's behaviour.
fn is_representation_item(item: &syn::ImplItem) -> bool {
    match item {
        syn::ImplItem::Type(t) => t.ident == "Type",
        syn::ImplItem::Fn(f) => matches!(
            f.sig.ident.to_string().as_str(),
            "type_matches_value" | "as_type_compiled_dyn" | "is_special" | "get_typed"
        ),
        _ => false,
    }
}

/// The shape of the generics on the impl's `Self`; see `ImplStarlarkValue::vtable_registration`.
#[derive(Copy, Clone, PartialEq, Eq)]
enum GenericsShape {
    /// No generics anywhere, e.g. `Foo`.
    None,
    /// Lifetime parameters only, e.g. `Foo<'v>` or `FooGen<Bar<'v>>`.
    LifetimesOnly,
    /// Anything else: type or const params.
    Other,
}

fn is_impl_starlark_value(
    input: syn::ItemImpl,
    attrs: StarlarkValueAttrs,
) -> syn::Result<ImplStarlarkValue> {
    let err = "expected `impl StarlarkValue for ...` or `impl StarlarkType for ...`";
    let Some((path, _)) = &input.trait_ else {
        return Err(syn::Error::new_spanned(input, err));
    };
    let Some(last) = path.segments.last() else {
        return Err(syn::Error::new_spanned(path, err));
    };
    let impl_of = if last.ident == "StarlarkValue" {
        ImplOf::Value
    } else if last.ident == "StarlarkType" {
        ImplOf::Type
    } else {
        return Err(syn::Error::new_spanned(&last.ident, err));
    };
    let lifetime_param = match find_v_lifetime(&input.generics)? {
        Some(lt) => lt.clone(),
        None => {
            return Err(syn::Error::new_spanned(
                input,
                "expected a lifetime parameter",
            ));
        }
    };
    Ok(ImplStarlarkValue {
        input,
        impl_of,
        lifetime_param,
        attrs,
    })
}

impl ImplStarlarkValue {
    fn please_use_starlark_type_macro(&self) -> syn::Result<syn::ImplItem> {
        syn::parse2(quote! {
            fn please_use_starlark_type_macro() {}
        })
    }

    fn typ(&self) -> syn::Result<&syn::Expr> {
        self.attrs.typ.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(&self.input.self_ty, "`type = \"...\"` is required")
        })
    }

    fn const_type(&self) -> syn::Result<syn::ImplItem> {
        let typ = self.typ()?;
        syn::parse2(quote! {
            const TYPE: &'static str = #typ;
        })
    }

    /// `get_type_starlark_repr` when the block does not override it: the nominal type, named
    /// through the representation (for a view, through its canonical representation, which
    /// `#[derive(StarlarkRepresentations)]` picks).
    fn get_type_starlark_repr(&self) -> syn::Result<Option<syn::ImplItem>> {
        if self.has_fn("get_type_starlark_repr") {
            return Ok(None);
        }
        let nominal = self.nominal_ty();
        Ok(Some(syn::parse_quote! {
            fn get_type_starlark_repr() -> starlark::typing::Ty {
                #nominal
            }
        }))
    }

    /// The nominal type as a `Ty`, named through the representation (for a view, through its
    /// canonical representation, which `#[derive(StarlarkRepresentations)]` picks).
    fn nominal_ty(&self) -> syn::Expr {
        let lt = &self.lifetime_param;
        match self.impl_of {
            ImplOf::Value => syn::parse_quote! {
                starlark::typing::Ty::starlark_value::<Self>()
            },
            ImplOf::Type => syn::parse_quote! {
                starlark::typing::Ty::starlark_value::<
                    <Self as starlark::__derive_refs::ViewOfRepresentations<#lt>>::Canonical,
                >()
            },
        }
    }

    /// A view is its own handle, so it is a union payload as itself.
    fn union_payload_is_self(&self) -> proc_macro2::TokenStream {
        let lt = &self.lifetime_param;
        let params = &self.input.generics.params;
        let where_clause = &self.input.generics.where_clause;
        let self_ty = &self.input.self_ty;
        quote! {
            impl<#params> starlark::values::type_expr::UnionPayload<#lt> for #self_ty
            #where_clause
            {
                type Expr = Self;

                #[inline]
                fn from_value(value: starlark::values::Value<#lt>) -> Option<Self> {
                    <Self as starlark::values::StarlarkTypeExpr<#lt>>::get(value)
                }
            }
        }
    }

    /// Whether the block writes the type's `Get` and `get` itself, as a type does whose
    /// representations are not a derived view.
    fn writes_get(&self) -> bool {
        self.input.items.iter().any(is_get_item)
    }

    /// `type Get` and `fn get` for a block that does not write them: the handle a value of
    /// the type is read through.
    ///
    /// `get` is `#[inline(always)]` as every step of a conversion is; `unpack_then` in
    /// `starlark` says why.
    fn default_get_items(&self) -> Vec<syn::ImplItem> {
        let lt = &self.lifetime_param;
        match self.impl_of {
            ImplOf::Value => vec![
                syn::parse_quote! {
                    type Get = starlark::values::ValueTyped<#lt, Self>;
                },
                syn::parse_quote! {
                    #[inline(always)]
                    fn get(value: starlark::values::Value<#lt>) -> Option<Self::Get> {
                        starlark::values::ValueTyped::new(value)
                    }
                },
            ],
            ImplOf::Type => vec![
                syn::parse_quote! {
                    type Get = Self;
                },
                syn::parse_quote! {
                    #[inline(always)]
                    fn get(value: starlark::values::Value<#lt>) -> Option<Self::Get> {
                        <Self as starlark::__derive_refs::ViewOfRepresentations<#lt>>::from_value(value)
                    }
                },
            ],
        }
    }

    /// The type as a type expression: the `StarlarkTypeExpr` and `StarlarkTypeExprStable`
    /// impls that `StarlarkType` has as its supertraits. `Get` and `get` are `get_items`, the
    /// block's own or the default; the `Ty` is the type's.
    fn type_expr_impl(
        &self,
        get_items: Vec<syn::ImplItem>,
    ) -> syn::Result<proc_macro2::TokenStream> {
        if self.attrs.type_expr_by_hand {
            return match get_items.first() {
                None => Ok(quote! {}),
                Some(item) => Err(syn::Error::new_spanned(
                    item,
                    "with `type_expr = by_hand` the type's `Get` and `get` are in the \
                     `StarlarkTypeExpr` impl it writes",
                )),
            };
        }
        let lt = &self.lifetime_param;
        let (impl_generics, _, where_clause) = self.input.generics.split_for_impl();
        let self_ty = &self.input.self_ty;
        let get_items = if get_items.is_empty() {
            self.default_get_items()
        } else {
            get_items
        };
        Ok(quote! {
            impl #impl_generics starlark::values::StarlarkTypeExpr<#lt> for #self_ty
            #where_clause
            {
                #( #get_items )*

                #[inline]
                fn starlark_type_repr() -> starlark::typing::Ty {
                    <Self as starlark::values::StarlarkType<#lt>>::get_type_starlark_repr()
                }
            }

            impl #impl_generics starlark::values::StarlarkTypeExprStable<#lt> for #self_ty
            #where_clause
            {
            }
        })
    }

    /// `module` is a `#[starlark_module]` function; that macro declares, next to it, statics
    /// named after it. This is the path of one of them.
    fn module_static(module: &syn::Path, suffix: &str) -> syn::Path {
        let mut path = module.clone();
        let last = path.segments.last_mut().expect("a path has a segment");
        last.ident = quote::format_ident!(
            "{}_{}",
            last.ident.to_string().to_uppercase(),
            suffix,
            span = last.ident.span()
        );
        path
    }

    /// For `methods = ..`: the table of the type's members as its `MEMBERS`, and a
    /// `get_methods` that is filled from the same table. Also the items that go before the
    /// block, which a type with two modules has: the table of both, and what `get_methods`
    /// returns.
    fn methods(&self) -> (proc_macro2::TokenStream, Vec<syn::ImplItem>) {
        let (before, members, statics): (proc_macro2::TokenStream, syn::Path, syn::Path) =
            match self.attrs.methods.as_slice() {
                [] => return (proc_macro2::TokenStream::new(), Vec::new()),
                [module] => (
                    proc_macro2::TokenStream::new(),
                    Self::module_static(module, "MEMBERS"),
                    Self::module_static(module, "STATICS"),
                ),
                modules => {
                    let (first, second) = (&modules[0], &modules[1]);
                    // The two statics need names that no other block of the module has.
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    std::hash::Hash::hash(
                        &self.input.self_ty.to_token_stream().to_string(),
                        &mut hasher,
                    );
                    let unique = std::hash::Hasher::finish(&hasher);
                    let members = quote::format_ident!("__STARLARK_MEMBERS_{:016X}", unique);
                    let statics = quote::format_ident!("__STARLARK_METHODS_{:016X}", unique);
                    let (first_members, second_members) = (
                        Self::module_static(first, "MEMBERS"),
                        Self::module_static(second, "MEMBERS"),
                    );
                    let (first_slots, second_slots) = (
                        Self::module_static(first, "SLOTS"),
                        Self::module_static(second, "SLOTS"),
                    );
                    (
                        quote! {
                            static #members: starlark::environment::MemberTable<
                                [starlark::environment::Member;
                                    starlark::environment::concat_slots(#first_slots, #second_slots)],
                            > = starlark::environment::MemberTable::concat(
                                &#first_members,
                                &#second_members,
                            );
                            starlark::methods_static!(
                                #statics = |builder| builder.extend(&#members)
                            );
                        },
                        syn::parse_quote! { #members },
                        syn::parse_quote! { #statics },
                    )
                }
            };
        (
            before,
            vec![
                syn::parse_quote! {
                    const MEMBERS: std::option::Option<&'static starlark::environment::MemberTable> =
                        std::option::Option::Some(&#members);
                },
                syn::parse_quote! {
                    fn get_methods() -> std::option::Option<&'static starlark::environment::Methods> {
                        std::option::Option::Some(#statics.methods())
                    }
                },
            ],
        )
    }

    fn get_type_value_static(&self) -> syn::Result<syn::ImplItem> {
        let typ = self.typ()?;
        syn::parse2(quote! {
            #[inline]
            fn get_type_value_static() -> starlark::values::StringValue<'static> {
                starlark::const_frozen_string!(#typ)
            }
        })
    }

    /// Generate `HAS_foo` flags for each implemented function, including generated by this macro.
    fn has_fn_flags(&self, fixed_trait: &syn::ItemImpl) -> syn::Result<Vec<syn::ImplItem>> {
        let mut flags = Vec::new();
        for item in &fixed_trait.items {
            if let syn::ImplItem::Fn(f) = item {
                let has_name = vtable_has_field_name(&f.sig.ident);
                flags.push(syn::parse_quote! {
                    const #has_name: bool = true;
                });
            }
        }
        // The vtable entries of `repr` and `str` read these two flags to choose between the
        // method and the representation's `Display`, and a macro may define either method.
        if self.has_macro_item() {
            for name in ["collect_repr", "collect_str"] {
                let written = fixed_trait
                    .items
                    .iter()
                    .any(|item| matches!(item, syn::ImplItem::Fn(f) if f.sig.ident == name));
                if !written {
                    let has_name = vtable_has_field_name(&quote::format_ident!("{}", name));
                    flags.push(syn::parse_quote! {
                        const #has_name: bool = true;
                    });
                }
            }
        }
        Ok(flags)
    }

    /// Find a function with given name.
    fn find_fn(&self, name: &str) -> Option<syn::ImplItemFn> {
        self.input.items.iter().find_map(|item| {
            if let syn::ImplItem::Fn(fn_) = item {
                if fn_.sig.ident == name {
                    Some(fn_.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    /// There's an function with given name.
    fn has_fn(&self, name: &str) -> bool {
        self.find_fn(name).is_some()
    }

    /// The impl block contains a macro invocation in item position.
    ///
    /// Attribute macros run before item macros are expanded, so we cannot see which
    /// `StarlarkValue` methods such a macro is going to define. Whenever the answer to
    /// "does this impl define `foo`?" would change the code we generate, we have to
    /// assume the macro might define it.
    fn has_macro_item(&self) -> bool {
        self.input
            .items
            .iter()
            .any(|item| matches!(item, syn::ImplItem::Macro(_)))
    }

    /// The arm of `bin_op_signatures` or `rbin_op_signatures` for an operator, if the block has
    /// its method: what the method's types say if it is written with them, any operand and an
    /// unknown result if it is not.
    fn operator_arm(
        &self,
        bin_op: &str,
        impl_name: &str,
        typed: &TypedMethods,
    ) -> Option<syn::Arm> {
        let bin_op = syn::Ident::new(bin_op, proc_macro2::Span::call_site());
        let any: syn::Expr = syn::parse_quote! { starlark::typing::OperatorSignature::any() };
        // A type that is evaluated as a type has `|` with another type, whatever its own
        // `bit_or` takes.
        let type_union = impl_name == "bit_or" && self.has_fn("eval_type");
        let mut signatures: Vec<syn::Expr> = Vec::new();
        match typed.get(impl_name) {
            Some(Typing::Operator { operand, result }) => {
                signatures.push(syn::parse_quote! {
                    starlark::__derive_refs::behaviour::operator_signature::<#operand, #result>()
                });
                signatures.extend(type_union.then_some(any));
            }
            Some(Typing::Compare { operand }) => signatures.push(syn::parse_quote! {
                starlark::__derive_refs::behaviour::compare_signature::<#operand>()
            }),
            Some(Typing::IsIn { operand }) => signatures.push(syn::parse_quote! {
                starlark::__derive_refs::behaviour::is_in_signature::<#operand>()
            }),
            Some(Typing::UnaryOperator { .. }) => {}
            None if self.has_fn(impl_name) || type_union => signatures.push(any),
            None => {}
        }
        if signatures.is_empty() {
            return None;
        }
        Some(syn::parse_quote! {
            starlark::typing::TypingBinOp::#bin_op => {
                std::vec::Vec::from([#( #signatures ),*])
            }
        })
    }

    /// `fn bin_op_signatures()` or `fn rbin_op_signatures()` from the arms of the operators
    /// the block has; nothing, which leaves the trait's default, if it has none.
    ///
    /// `every_operator` says that `arms` has an entry for every `TypingBinOp`.
    fn operator_signatures(
        name: &str,
        arms: Vec<Option<syn::Arm>>,
        every_operator: bool,
    ) -> Option<syn::ImplItem> {
        if arms.iter().all(Option::is_none) {
            return None;
        }
        let name = syn::Ident::new(name, proc_macro2::Span::call_site());
        let default_arm: Option<syn::Arm> = if every_operator && arms.iter().all(Option::is_some) {
            None
        } else {
            Some(syn::parse_quote! {
                _ => { Vec::new() }
            })
        };
        let arms = arms.into_iter().flatten();
        Some(syn::parse_quote! {
            fn #name(op: starlark::typing::TypingBinOp) -> Vec<starlark::typing::OperatorSignature> {
                match op {
                    #( #arms )*
                    #default_arm
                }
            }
        })
    }

    /// `fn bin_op_signatures()`, unless the block writes it.
    fn bin_op_signatures(&self, typed: &TypedMethods) -> Option<syn::ImplItem> {
        if self.has_fn("bin_op_signatures") {
            return None;
        }
        Self::operator_signatures(
            "bin_op_signatures",
            vec![
                self.operator_arm("Add", "add", typed),
                self.operator_arm("Sub", "sub", typed),
                self.operator_arm("Mul", "mul", typed),
                self.operator_arm("Div", "div", typed),
                self.operator_arm("FloorDiv", "floor_div", typed),
                self.operator_arm("Percent", "percent", typed),
                self.operator_arm("In", "is_in", typed),
                self.operator_arm("BitOr", "bit_or", typed),
                self.operator_arm("BitAnd", "bit_and", typed),
                self.operator_arm("BitXor", "bit_xor", typed),
                self.operator_arm("Less", "compare", typed),
                self.operator_arm("LeftShift", "left_shift", typed),
                self.operator_arm("RightShift", "right_shift", typed),
            ],
            true,
        )
    }

    /// `fn rbin_op_signatures()`, unless the block writes it.
    fn rbin_op_signatures(&self, typed: &TypedMethods) -> Option<syn::ImplItem> {
        if self.has_fn("rbin_op_signatures") {
            return None;
        }
        Self::operator_signatures(
            "rbin_op_signatures",
            vec![
                self.operator_arm("Add", "radd", typed),
                self.operator_arm("Mul", "rmul", typed),
            ],
            false,
        )
    }

    /// `fn un_op_result()`, unless the block writes it: for each unary operator the block has,
    /// the type of its result if it is written with one, the type itself if it is not.
    fn un_op_result(&self, typed: &TypedMethods) -> Option<syn::ImplItem> {
        if self.has_fn("un_op_result") {
            return None;
        }
        let nominal = self.nominal_ty();
        let arms: Vec<syn::Arm> = [("Plus", "plus"), ("Minus", "minus"), ("BitNot", "bit_not")]
            .into_iter()
            .filter(|(_, impl_name)| self.has_fn(impl_name))
            .map(|(un_op, impl_name)| {
                let un_op = syn::Ident::new(un_op, proc_macro2::Span::call_site());
                let result: syn::Expr = match typed.get(impl_name) {
                    Some(Typing::UnaryOperator { result }) => syn::parse_quote! {
                        starlark::__derive_refs::behaviour::unary_operator_result::<#result>()
                    },
                    _ => nominal.clone(),
                };
                syn::parse_quote! {
                    starlark::typing::TypingUnOp::#un_op => Some(#result),
                }
            })
            .collect();
        if arms.is_empty() {
            return None;
        }
        let default_arm: Option<syn::Arm> = (arms.len() != 3).then(|| {
            syn::parse_quote! {
                _ => None,
            }
        });
        Some(syn::parse_quote! {
            fn un_op_result(op: starlark::typing::TypingUnOp) -> Option<starlark::typing::Ty> {
                match op {
                    #( #arms )*
                    #default_arm
                }
            }
        })
    }

    /// `fn at_signature()`, unless the block writes it: for an `at` written with types, what
    /// they say; for one written as the trait declares it, any index and an unknown result.
    fn at_signature(&self, typed: &TypedMethods) -> Option<syn::ImplItem> {
        if self.has_fn("at_signature") || !self.has_fn("at") {
            return None;
        }
        let signature: syn::Expr = match typed.get("at") {
            Some(Typing::Operator { operand, result }) => syn::parse_quote! {
                starlark::__derive_refs::behaviour::operator_signature::<#operand, #result>()
            },
            _ => syn::parse_quote! { starlark::typing::OperatorSignature::any() },
        };
        Some(syn::parse_quote! {
            fn at_signature() -> Option<starlark::typing::OperatorSignature> {
                Some(#signature)
            }
        })
    }

    /// `fn attr_ty()`.
    fn attr_ty(&self) -> syn::Result<Option<syn::ImplItem>> {
        if self.has_fn("attr_ty") || self.has_macro_item() {
            // User has custom `attr_ty` implementation, or an item macro which may
            // expand to `get_attr`/`attr_ty`.
            // Fall back to the default implementation, which returns `Any` for every
            // attribute: we cannot prove the attribute does not exist.
            Ok(None)
        } else if !self.has_fn("get_attr") {
            Ok(Some(syn::parse2(quote! {
                fn attr_ty(_attr: &str) -> ::std::option::Option<starlark::typing::Ty> {
                    ::std::option::Option::None
                }
            })?))
        } else {
            // Use default implementation which returns `Any`.
            Ok(None)
        }
    }

    /// `fn bit_or()` for values which are types.
    fn bit_or(&self) -> syn::Result<Option<syn::ImplItem>> {
        if !self.has_fn("eval_type") {
            return Ok(None);
        }
        if let Some(bit_or) = self.find_fn("bit_or") {
            return Err(syn::Error::new_spanned(
                bit_or.sig.ident,
                "types with `eval_type` implemented can only have generated `bit_or`",
            ));
        }
        Ok(Some(syn::parse2(quote! {
            fn bit_or(this: Self::Get, other: starlark::values::Value<'v>, heap: starlark::values::Heap<'v>) -> starlark::Result<starlark::values::Value<'v>> {
                starlark::values::typing::macro_refs::starlark_value_bit_or_for_type::<Self>(this, other, heap)
            }
        })?))
    }

    /// Classify `Self`'s generics for `vtable_registration`.
    fn generics_shape(&self) -> GenericsShape {
        let lifetimes_only = self
            .input
            .generics
            .params
            .iter()
            .all(|param| matches!(param, syn::GenericParam::Lifetime(_)));
        if !lifetimes_only {
            return GenericsShape::Other;
        }
        let self_has_args = matches!(&*self.input.self_ty, syn::Type::Path(p)
            if p.path.segments.last()
                .map(|s| !matches!(s.arguments, syn::PathArguments::None))
                .unwrap_or(false));
        if self_has_args {
            GenericsShape::LifetimesOnly
        } else {
            GenericsShape::None
        }
    }

    /// Emit registrations for vtable lookup. The shape of `Self`'s generics
    /// determines which instantiations are registered:
    ///
    /// | Generics on `Self`                   | AValue (frozen)            | Ty                                                     |
    /// |--------------------------------------|----------------------------|--------------------------------------------------------|
    /// | none, e.g. `Foo`                     | `Foo`                      | `Foo`                                                  |
    /// | lifetimes only, e.g. `Foo<'v>`       | none                       | `Foo<'_>`                                              |
    /// | lifetimes only + `frozen_vtable`     | `Foo<'static>`             | `Foo<'_>`                                              |
    /// | otherwise (const or type params)     | none: the caller registers each instantiation manually |
    ///
    /// The lifetimes-only row is the shape both of a type that freezes to a different type
    /// and of one that is its own frozen form; only the flag tells them apart.
    /// For the first two rows, `skip_vtable` drops all registrations and
    /// `no_freeze` the AValue one.
    ///
    /// `is_special` (user-defined `fn is_special`) suppresses the AValue vtable
    /// emit — the user is expected to register AValue externally.
    fn vtable_registration(&self) -> syn::Result<Option<proc_macro2::TokenStream>> {
        let is_special = self.has_fn("is_special");
        let shape = self.generics_shape();
        let frozen_vtable_err = |msg: &str| Err(syn::Error::new_spanned(&self.input.self_ty, msg));

        match (shape, self.attrs.vtable) {
            (GenericsShape::Other, VtableMode::Frozen) => frozen_vtable_err(
                "`frozen_vtable` needs a self type that is concrete apart from lifetimes; \
                 register the frozen instantiations with `register_simple_vtable_entry!`",
            ),
            (GenericsShape::Other, _) => Ok(None),
            (GenericsShape::None, VtableMode::Frozen) => frozen_vtable_err(
                "`frozen_vtable` is redundant: a type without lifetime parameters is \
                 registered as its own frozen form by default",
            ),
            (GenericsShape::LifetimesOnly, VtableMode::Frozen) => {
                if is_special {
                    return frozen_vtable_err(
                        "`frozen_vtable` cannot be combined with a custom `is_special`, \
                         whose `AValue` vtable is registered externally",
                    );
                }
                Ok(Some(self.emit_frozen_vtable_for_lifetimes_only()))
            }
            (_, VtableMode::Skip) => Ok(None),
            (_, VtableMode::TyOnly) | (GenericsShape::LifetimesOnly, VtableMode::Default) => {
                // Statics can't carry a real `'v`, so the Ty registration elides the
                // lifetimes to `'_`.
                let elided = self.self_ty_with_lifetime(syn::parse_quote! { '_ });
                Ok(Some(quote! {
                    starlark::register_ty_starlark_value!(#elided);
                }))
            }
            (GenericsShape::None, VtableMode::Default) => {
                let frozen_ty = (*self.input.self_ty).clone();
                Ok(Some(self.emit_frozen_avalue_vtable_and_ty_vtable(
                    is_special, &frozen_ty,
                )))
            }
        }
    }

    /// Emit AValue vtable + Ty vtable registrations for the frozen form.
    /// AValue is suppressed when the user supplies a custom `is_special`.
    fn emit_frozen_avalue_vtable_and_ty_vtable(
        &self,
        is_special: bool,
        frozen_ty: &syn::Type,
    ) -> proc_macro2::TokenStream {
        let avalue = if is_special {
            proc_macro2::TokenStream::new()
        } else {
            quote! {
                starlark::register_simple_vtable_entry!(#frozen_ty);
                unsafe impl starlark::__derive_refs::VtableRegistered for #frozen_ty {}
            }
        };
        quote! {
            #avalue
            starlark::register_ty_starlark_value!(#frozen_ty);
        }
    }

    /// Emit the registrations for a lifetimes-only `Self` that is its own frozen form.
    fn emit_frozen_vtable_for_lifetimes_only(&self) -> proc_macro2::TokenStream {
        let frozen_ty = self.self_ty_with_lifetime(syn::parse_quote! { 'static });
        let elided_ty = self.self_ty_with_lifetime(syn::parse_quote! { '_ });
        let self_ty = &self.input.self_ty;
        let (impl_generics, _, where_clause) = self.input.generics.split_for_impl();
        // The vtable entry is keyed by a lifetime-erased type id, so registering the
        // `'static` instantiation serves the frozen form at every heap lifetime, which is
        // what justifies making the `VtableRegistered` promise for all of them.
        quote! {
            starlark::register_simple_vtable_entry!(#frozen_ty);
            unsafe impl #impl_generics starlark::__derive_refs::VtableRegistered for #self_ty #where_clause {}
            starlark::register_ty_starlark_value!(#elided_ty);
        }
    }

    /// The `AllocRepresentation` and `AllocFrozenRepresentation` impls for `Self`, from
    /// `alloc = ...`, or failing that from the same shape and flags as the vtable
    /// registration: a type without lifetimes is simple, one with lifetimes is complex, a
    /// `frozen_vtable` type is a frozen form, `no_freeze` and `is_special` say what they say.
    /// `skip_vtable` on a type without lifetimes is an unfrozen type whose frozen form is
    /// another type, so it is complex too.
    fn alloc_impls(&self) -> proc_macro2::TokenStream {
        if self.impl_of == ImplOf::Type || self.has_fn("is_special") {
            return proc_macro2::TokenStream::new();
        }
        let mode = match self.attrs.alloc {
            AllocMode::Default => match (self.generics_shape(), self.attrs.vtable) {
                (GenericsShape::Other, _) => return proc_macro2::TokenStream::new(),
                (GenericsShape::None, VtableMode::Default) => AllocMode::Simple,
                (GenericsShape::None, VtableMode::Skip)
                | (GenericsShape::LifetimesOnly, VtableMode::Default | VtableMode::Skip) => {
                    AllocMode::Complex
                }
                (_, VtableMode::TyOnly) => AllocMode::ComplexNoFreeze,
                (_, VtableMode::Frozen) => AllocMode::Frozen,
            },
            mode => mode,
        };
        let (unfrozen, frozen): (Option<syn::Ident>, bool) = match mode {
            AllocMode::ByHand => return proc_macro2::TokenStream::new(),
            AllocMode::Simple => (Some(syn::parse_quote!(alloc_simple)), true),
            AllocMode::Complex => (
                Some(syn::parse_quote!(alloc_complex)),
                self.attrs.vtable == VtableMode::Frozen,
            ),
            AllocMode::ComplexNoFreeze => (Some(syn::parse_quote!(alloc_complex_no_freeze)), false),
            AllocMode::Frozen => (None, true),
            AllocMode::Default => unreachable!(),
        };
        // The impl's own `where` clause is not copied: a `where Self: ProvidesStaticType<'v>`
        // would make the static type opaque inside the impl and fail the `for<'a>` bound of
        // `alloc_simple`, and a type with only lifetime parameters needs no clause here.
        let lt = &self.lifetime_param;
        let params = &self.input.generics.params;
        let self_ty = &self.input.self_ty;
        let unfrozen = unfrozen.map(|strategy| {
            quote! {
                impl<#params> starlark::values::AllocRepresentation<#lt> for #self_ty {
                    type Repr = Self;

                    #[inline]
                    fn alloc(
                        self,
                        allocator: starlark::values::Allocator<#lt>,
                    ) -> starlark::values::ValueTyped<#lt, Self> {
                        allocator.#strategy(self)
                    }
                }
            }
        });
        let frozen = frozen.then(|| {
            quote! {
                impl<#params> starlark::values::AllocFrozenRepresentation<#lt> for #self_ty {
                    type Repr = Self;

                    #[inline]
                    fn alloc_frozen(
                        self,
                        allocator: starlark::values::FrozenAllocator<#lt>,
                    ) -> starlark::values::ValueTyped<#lt, Self> {
                        allocator.alloc_simple(self)
                    }
                }
            }
        });
        quote! { #unfrozen #frozen }
    }

    /// `Self` with every lifetime replaced by `lifetime`, e.g. `Foo<'v>` → `Foo<'static>`.
    fn self_ty_with_lifetime(&self, lifetime: syn::Lifetime) -> syn::Type {
        struct ReplaceLifetimes(syn::Lifetime);

        impl syn::visit_mut::VisitMut for ReplaceLifetimes {
            fn visit_lifetime_mut(&mut self, i: &mut syn::Lifetime) {
                *i = self.0.clone();
            }
        }

        let mut ty = (*self.input.self_ty).clone();
        syn::visit_mut::VisitMut::visit_type_mut(&mut ReplaceLifetimes(lifetime), &mut ty);
        ty
    }
}

impl ImplStarlarkValue {
    /// The `impl StarlarkType` for `self_ty`: `items`, a method written with a typed signature
    /// replaced by the trait's method derived from it, plus the generated members. Next to it,
    /// what the typed methods say about the type.
    fn type_impl(&self, items: Vec<syn::ImplItem>) -> syn::Result<proc_macro2::TokenStream> {
        let lt = &self.lifetime_param;
        let (get_items, mut items): (Vec<syn::ImplItem>, Vec<syn::ImplItem>) =
            items.into_iter().partition(is_get_item);
        let type_expr_impl = self.type_expr_impl(get_items)?;
        let memberships = take_memberships(&mut items)?;
        let mut always_hashes = None;
        let mut typed = TypedMethods::new();
        for item in &mut items {
            if let syn::ImplItem::Fn(f) = item {
                if let Some(derived) = derive_from_typed(f, lt)? {
                    *f = derived.method;
                    if let Some(typing) = derived.typing {
                        typed.insert(f.sig.ident.to_string(), typing);
                    }
                    // A view's method is inlined into each representation's vtable entry (see
                    // below), and for a derived comparison the hint is not enough: converting
                    // the operand makes it look larger than it is once the receiver's variant
                    // is known. Left to the hint, the list view's `compare` stays out of line
                    // and dispatches on the variant when it runs: 3.6% more instructions on
                    // `xs < ys`, with the `starlark` crate built as one codegen unit.
                    //
                    // `at`, `is_in` and `length` are forced with them. The hint is enough for
                    // the list and tuple views, and leaves the enum type view's `at` out of
                    // line: 1.8% more instructions on `Enum[i]`.
                    //
                    // An operator is left to the hint. Forced, the conversion goes into each
                    // entry and the body, which allocates and is not small, stays behind one
                    // more call: 2.5% more instructions on `xs * 3`.
                    if self.impl_of == ImplOf::Type
                        && derived.small
                        && !f.attrs.iter().any(|a| a.path().is_ident("inline"))
                    {
                        f.attrs.push(syn::parse_quote! { #[inline(always)] });
                    }
                    if derived.always_hashes {
                        always_hashes = Some(always_hashes_impl(&self.input, lt));
                    }
                }
            }
        }
        for item in &mut items {
            if let syn::ImplItem::Fn(f) = item {
                self.bind_receiver(f)?;
            }
        }
        let mut type_impl = self.input.clone();
        if self.impl_of == ImplOf::Value {
            let (_, for_token) = type_impl.trait_.take().expect("checked to be a trait impl");
            type_impl.trait_ = Some((
                syn::parse_quote! { starlark::values::StarlarkType<#lt> },
                for_token,
            ));
        }
        type_impl.items = items;
        type_impl.items.splice(
            0..0,
            [self.const_type()?, self.get_type_value_static()?]
                .into_iter()
                .chain(self.methods().1)
                .chain(self.get_type_starlark_repr()?)
                .chain(self.attr_ty()?)
                .chain(self.bit_or()?)
                .chain(self.bin_op_signatures(&typed))
                .chain(self.rbin_op_signatures(&typed))
                .chain(self.un_op_result(&typed))
                .chain(self.at_signature(&typed)),
        );
        let has_fn_flags = self.has_fn_flags(&type_impl)?;
        type_impl.items.extend(has_fn_flags);
        // A method is the body of a vtable entry and is hinted into it. For a view, each
        // representation's entry builds the view with a constant variant; inlined there, the
        // dispatch on the variant folds away and the code is what one `impl` per representation
        // would have been. For a type that is its own representation the entry is the method's
        // one caller, and left out of line it is a frame around a call wherever the result
        // comes back through memory: 7 instructions on `x in range(..)`.
        for item in &mut type_impl.items {
            if let syn::ImplItem::Fn(f) = item {
                if !f.attrs.iter().any(|a| a.path().is_ident("inline")) {
                    f.attrs.push(syn::parse_quote! { #[inline] });
                }
            }
        }
        let memberships = memberships_impl(&memberships, &self.input, lt);
        let before = self.methods().0;
        Ok(quote! {
            #before

            #type_impl

            #type_expr_impl

            #always_hashes

            #memberships
        })
    }

    /// `get_typed` for a representation whose `Type` is a view that
    /// `#[derive(StarlarkRepresentations)]` wrote: the view of this representation.
    fn get_typed_via_from(&self) -> syn::ImplItem {
        let lt = &self.lifetime_param;
        syn::parse_quote! {
            #[inline]
            fn get_typed(
                this: starlark::values::ValueTyped<#lt, Self>,
            ) -> <Self::Type as starlark::values::StarlarkTypeExpr<#lt>>::Get {
                <Self::Type as std::convert::From<starlark::values::ValueTyped<#lt, Self>>>::from(this)
            }
        }
    }

    /// `get_typed` for a representation that is its own type: the handle to the
    /// representation is the type's `Get`. A block that writes a `Get` of its own writes
    /// `get_typed` as well.
    fn get_typed_identity(&self) -> syn::Result<Option<syn::ImplItem>> {
        if self.writes_get() {
            return Ok(None);
        }
        let lt = &self.lifetime_param;
        Ok(Some(syn::parse_quote! {
            #[inline]
            fn get_typed(
                this: starlark::values::ValueTyped<#lt, Self>,
            ) -> <Self::Type as starlark::values::StarlarkTypeExpr<#lt>>::Get {
                this
            }
        }))
    }

    /// A method of the block as the method of `StarlarkType`, which takes the type's `Get`.
    ///
    /// The method's first parameter is its receiver. It is named `this` and declared with any
    /// type that converts from the type; the body runs with it bound from the `Get`. A
    /// receiver declared `Self::Get` is the trait's own, and the method stays as written.
    fn bind_receiver(&self, f: &mut syn::ImplItemFn) -> syn::Result<()> {
        if WITHOUT_RECEIVER.iter().any(|name| f.sig.ident == name) {
            return Ok(());
        }
        let (binding, this): (syn::Stmt, syn::Ident) = match f.sig.inputs.first() {
            Some(syn::FnArg::Typed(param)) if is_named_this(&param.pat) => {
                if is_self_get(&param.ty) {
                    return Ok(());
                }
                let this = syn::Ident::new("__starlark_this", param.pat.span());
                let pat = &param.pat;
                let ty = &param.ty;
                // Spanned to the parameter, which is what a receiver type that does not
                // convert from the type is an error about.
                let binding = syn::parse_quote_spanned! { param.span() =>
                    let #pat: #ty = starlark::__derive_refs::behaviour::receiver::<Self, _>(#this);
                };
                (binding, this)
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &f.sig,
                    "the first parameter of a `StarlarkType` method is its receiver: a parameter \
                     named `this` of a type that converts from the type, such as `this: &Self`",
                ));
            }
        };
        f.sig.inputs[0] = syn::parse_quote! { #this: Self::Get };
        let body = &f.block;
        f.block = syn::parse_quote! {
            {
                #binding
                #body
            }
        };
        Ok(())
    }
}

/// The functions of `StarlarkType` that are not about a value. Every other function of the trait
/// takes the value as its first parameter.
const WITHOUT_RECEIVER: &[&str] = &[
    "get",
    "get_type_value_static",
    "get_type_starlark_repr",
    "get_methods",
    "attr_ty",
    "bin_op_signatures",
    "rbin_op_signatures",
    "un_op_result",
    "at_signature",
];

/// The parameter is named `this`, with or without the underscores of an unused one.
fn is_named_this(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Ident(pat) => pat.ident.to_string().trim_start_matches('_') == "this",
        _ => false,
    }
}

fn derive_starlark_value_impl(
    attr: StarlarkValueAttrs,
    input: syn::ItemImpl,
) -> syn::Result<proc_macro2::TokenStream> {
    let impl_starlark_value: ImplStarlarkValue = is_impl_starlark_value(input.clone(), attr)?;

    if impl_starlark_value.impl_of == ImplOf::Type {
        // A type whose representations are elsewhere: nothing is allocated as it, so there is
        // no vtable to register and nothing to unpack.
        if impl_starlark_value.attrs.vtable != VtableMode::Default {
            return Err(syn::Error::new_spanned(
                &input.self_ty,
                "an `impl StarlarkType` takes only `type = ...`; vtable registration is the \
                 representations'",
            ));
        }
        let type_impl = impl_starlark_value.type_impl(input.items.clone())?;
        let union_payload = if impl_starlark_value.attrs.type_expr_by_hand {
            None
        } else {
            Some(impl_starlark_value.union_payload_is_self())
        };
        return Ok(quote! { #type_impl #union_payload });
    }

    let (rep_items, ty_items): (Vec<syn::ImplItem>, Vec<syn::ImplItem>) = input
        .items
        .iter()
        .cloned()
        .partition(is_representation_item);
    let names_type = rep_items
        .iter()
        .any(|item| matches!(item, syn::ImplItem::Type(t) if t.ident == "Type"));

    let please_use_starlark_type_macro = impl_starlark_value.please_use_starlark_type_macro()?;
    let vtable_registration = impl_starlark_value.vtable_registration()?;
    let alloc_impls = impl_starlark_value.alloc_impls();
    let mut rep_impl = input.clone();
    rep_impl.items = rep_items;

    if names_type {
        // One representation of a type with several: its behaviour is the type's.
        if let Some(item) = ty_items.first() {
            return Err(syn::Error::new_spanned(
                item,
                "a representation of a type with several representations has no behaviour of \
                 its own; this belongs on the `impl StarlarkType` of its `Type`",
            ));
        }
        if let Some(typ) = &impl_starlark_value.attrs.typ {
            return Err(syn::Error::new_spanned(
                typ,
                "the `type()` string is the `Type`'s; it is not given on a representation",
            ));
        }
        let get_typed = if impl_starlark_value.has_fn("get_typed") {
            None
        } else {
            Some(impl_starlark_value.get_typed_via_from())
        };
        rep_impl.items.splice(
            0..0,
            [please_use_starlark_type_macro]
                .into_iter()
                .chain(get_typed),
        );
        return Ok(quote! {
            #rep_impl

            #vtable_registration

            #alloc_impls
        });
    }

    let type_impl = impl_starlark_value.type_impl(ty_items)?;
    let type_self: syn::ImplItem = syn::parse_quote! {
        type Type = Self;
    };
    let get_typed = impl_starlark_value.get_typed_identity()?;
    rep_impl.items.splice(
        0..0,
        [please_use_starlark_type_macro, type_self]
            .into_iter()
            .chain(get_typed),
    );

    Ok(quote! {
        #type_impl

        #rep_impl

        #vtable_registration

        #alloc_impls
    })
}
