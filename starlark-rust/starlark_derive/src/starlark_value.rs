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

use quote::quote;

use crate::util::GenericsUtil;
use crate::v_lifetime::find_v_lifetime;
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
    /// Accepted and does nothing: `UnpackValue for &T` is a blanket impl.
    unpack_value: bool,
    /// Implement `StarlarkTypeRepr` for `&T`.
    starlark_type_repr: bool,
    vtable: VtableMode,
    alloc: AllocMode,
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
            unpack_value: false,
            starlark_type_repr: false,
            vtable: VtableMode::Default,
            alloc: AllocMode::Default,
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
            if name == "UnpackValue" {
                attrs.unpack_value = true;
            } else if name == "StarlarkTypeRepr" {
                attrs.starlark_type_repr = true;
            } else if name == "skip_vtable" {
                attrs.set_vtable_mode(&name, VtableMode::Skip)?;
            } else if name == "no_freeze" {
                attrs.set_vtable_mode(&name, VtableMode::TyOnly)?;
                attrs.set_alloc_mode(&name, AllocMode::ComplexNoFreeze)?;
            } else if name == "frozen_vtable" {
                attrs.set_vtable_mode(&name, VtableMode::Frozen)?;
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
            } else {
                return Err(syn::Error::new_spanned(
                    name,
                    "unknown attribute, allowed attributes are `UnpackValue`, `StarlarkTypeRepr`, `skip_vtable`, `no_freeze`, `frozen_vtable`, `alloc = ...`",
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

struct ImplStarlarkValue {
    input: syn::ItemImpl,
    impl_of: ImplOf,
    lifetime_param: syn::Lifetime,
    attrs: StarlarkValueAttrs,
}

/// An item of an `impl StarlarkValue` block that stays on the representation. Everything else
/// is the type's behaviour.
fn is_representation_item(item: &syn::ImplItem) -> bool {
    match item {
        syn::ImplItem::Type(t) => t.ident == "Type",
        syn::ImplItem::Fn(f) => matches!(
            f.sig.ident.to_string().as_str(),
            "provide" | "type_matches_value" | "is_special" | "with_type"
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
    /// Impl `StarlarkTypeRepr for &T`.
    fn impl_unpack_value(&self) -> syn::Result<proc_macro2::TokenStream> {
        if !self.attrs.starlark_type_repr {
            return Ok(proc_macro2::TokenStream::new());
        }

        GenericsUtil::new(&self.input.generics).assert_only_lifetime_params()?;

        let lt = &self.lifetime_param;
        let params = &self.input.generics.params;
        // TODO(nga): where clause is incorrect:
        //   if there's something `Self: Xxx` constraint, it should be `*Self: Xxx`.
        let where_clause = &self.input.generics.where_clause;
        let self_ty = &self.input.self_ty;
        Ok(quote! {
            impl<#params> starlark::values::type_repr::StarlarkTypeRepr for &#lt #self_ty
            #where_clause
            {
                type Canonical = #self_ty;

                fn starlark_type_repr() -> starlark::typing::Ty {
                    <#self_ty as starlark::values::type_repr::StarlarkTypeRepr>::starlark_type_repr()
                }
            }
        })
    }

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
        Ok(Some(match self.impl_of {
            ImplOf::Value => syn::parse_quote! {
                fn get_type_starlark_repr() -> starlark::typing::Ty {
                    starlark::typing::Ty::starlark_value::<Self>()
                }
            },
            ImplOf::Type => syn::parse_quote! {
                fn get_type_starlark_repr() -> starlark::typing::Ty {
                    starlark::typing::Ty::starlark_value::<
                        <Self as starlark::values::type_repr::StarlarkTypeRepr>::Canonical,
                    >()
                }
            },
        }))
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

    /// `type Get` and `fn get`: the handle a value of the type is read through. A type whose
    /// representations are not a derived view writes its own.
    fn get_items(&self) -> syn::Result<Vec<syn::ImplItem>> {
        let lt = &self.lifetime_param;
        let written = self.input.items.iter().any(|item| match item {
            syn::ImplItem::Type(t) => t.ident == "Get",
            syn::ImplItem::Fn(f) => f.sig.ident == "get",
            _ => false,
        });
        if written {
            return Ok(Vec::new());
        }
        let items: Vec<syn::ImplItem> = match self.impl_of {
            ImplOf::Value => vec![
                syn::parse_quote! {
                    type Get = starlark::values::ValueTyped<#lt, Self>;
                },
                syn::parse_quote! {
                    #[inline]
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
                    #[inline]
                    fn get(value: starlark::values::Value<#lt>) -> Option<Self::Get> {
                        <Self as starlark::__derive_refs::ViewOfRepresentations<#lt>>::from_value(value)
                    }
                },
            ],
        };
        Ok(items)
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

    fn bin_op_arm(&self, bin_op: &str, impl_name: &str) -> Option<syn::Arm> {
        let bin_op = syn::Ident::new(bin_op, proc_macro2::Span::call_site());
        if self.has_fn(impl_name) || (impl_name == "bit_or" && self.has_fn("eval_type")) {
            Some(syn::parse_quote! {
                starlark::typing::TypingBinOp::#bin_op => {
                    Some(starlark::typing::Ty::any())
                }
            })
        } else {
            None
        }
    }

    fn bin_op_ty_impl(&self) -> syn::Result<Option<syn::ImplItem>> {
        let arms = [
            self.bin_op_arm("Add", "add"),
            self.bin_op_arm("Sub", "sub"),
            self.bin_op_arm("Mul", "mul"),
            self.bin_op_arm("Div", "div"),
            self.bin_op_arm("FloorDiv", "floor_div"),
            self.bin_op_arm("Percent", "percent"),
            self.bin_op_arm("In", "is_in"),
            self.bin_op_arm("BitOr", "bit_or"),
            self.bin_op_arm("BitAnd", "bit_and"),
            self.bin_op_arm("BitXor", "bit_xor"),
            self.bin_op_arm("Less", "compare"),
            self.bin_op_arm("LeftShift", "left_shift"),
            self.bin_op_arm("RightShift", "right_shift"),
        ];
        if arms.iter().all(Option::is_none) {
            // Use default implementation.
            return Ok(None);
        }
        let default_arm: Option<syn::Arm> = if arms.iter().all(Option::is_some) {
            None
        } else {
            Some(syn::parse_quote! {
                _ => { None }
            })
        };
        let arms = arms.into_iter().flatten();
        Ok(Some(syn::parse_quote! {
            fn bin_op_ty(op: starlark::typing::TypingBinOp, _rhs: &starlark::typing::TyBasic) -> Option<starlark::typing::Ty> {
                match op {
                    #( #arms )*
                    #default_arm
                }
            }
        }))
    }

    fn rbin_op_ty_impl(&self) -> syn::Result<Option<syn::ImplItem>> {
        let arms = [
            self.bin_op_arm("Add", "radd"),
            self.bin_op_arm("Mul", "rmul"),
        ];
        if arms.iter().all(Option::is_none) {
            // Use default implementation.
            return Ok(None);
        }
        Ok(Some(syn::parse_quote! {
            fn rbin_op_ty(_lhs: &starlark::typing::TyBasic, op: starlark::typing::TypingBinOp) -> Option<starlark::typing::Ty> {
                match op {
                    #( #arms )*
                    _ => {
                        None
                    }
                }
            }
        }))
    }

    /// Generate `fn bin_op_ty()`.
    fn bin_op_ty(&self) -> syn::Result<Option<syn::ImplItem>> {
        if self.has_fn("bin_op_ty") {
            Ok(None)
        } else {
            self.bin_op_ty_impl()
        }
    }

    /// Generate `fn rbin_op_ty()`.
    fn rbin_op_ty(&self) -> syn::Result<Option<syn::ImplItem>> {
        if self.has_fn("rbin_op_ty") {
            Ok(None)
        } else {
            self.rbin_op_ty_impl()
        }
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
            fn bit_or(&self, other: starlark::values::Value<'v>, heap: starlark::values::Heap<'v>) -> starlark::Result<starlark::values::Value<'v>> {
                starlark::values::typing::macro_refs::starlark_value_bit_or_for_type(self, other, heap)
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
    /// The `impl StarlarkType` for `self_ty`: `items` plus the generated members.
    fn type_impl(&self, items: Vec<syn::ImplItem>) -> syn::Result<syn::ItemImpl> {
        let lt = &self.lifetime_param;
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
                .chain(self.get_items()?)
                .chain(self.get_type_starlark_repr()?)
                .chain(self.attr_ty()?)
                .chain(self.bit_or()?)
                .chain(self.bin_op_ty()?)
                .chain(self.rbin_op_ty()?),
        );
        let has_fn_flags = self.has_fn_flags(&type_impl)?;
        type_impl.items.extend(has_fn_flags);
        if self.impl_of == ImplOf::Type {
            // A view's methods are the bodies of each representation's vtable entries, which
            // build the view with a constant variant; inlined there, the dispatch on the variant
            // folds away and the code is what one `impl` per representation would have been.
            for item in &mut type_impl.items {
                if let syn::ImplItem::Fn(f) = item {
                    if !f.attrs.iter().any(|a| a.path().is_ident("inline")) {
                        f.attrs.push(syn::parse_quote! { #[inline] });
                    }
                }
            }
        }
        Ok(type_impl)
    }

    /// `with_type` for a representation whose `Type` is a view that
    /// `#[derive(StarlarkRepresentations)]` wrote.
    fn with_type_via_from(&self) -> syn::ImplItem {
        let lt = &self.lifetime_param;
        syn::parse_quote! {
            #[inline]
            fn with_type<StarlarkWithTypeResult>(
                this: starlark::values::ValueTyped<#lt, Self>,
                f: impl FnOnce(&Self::Type) -> StarlarkWithTypeResult,
            ) -> StarlarkWithTypeResult {
                f(&<Self::Type as std::convert::From<starlark::values::ValueTyped<#lt, Self>>>::from(this))
            }
        }
    }

    /// `with_type` and `with_type_at` for a representation that is its own type.
    fn with_type_identity(&self) -> [syn::ImplItem; 2] {
        let lt = &self.lifetime_param;
        [
            syn::parse_quote! {
                #[inline]
                fn with_type<StarlarkWithTypeResult>(
                    this: starlark::values::ValueTyped<#lt, Self>,
                    f: impl FnOnce(&Self::Type) -> StarlarkWithTypeResult,
                ) -> StarlarkWithTypeResult {
                    f(this.as_ref())
                }
            },
            syn::parse_quote! {
                #[inline]
                fn with_type_at<StarlarkWithTypeResult>(
                    payload: &#lt Self,
                    _this: starlark::values::ValueTyped<#lt, Self>,
                    f: impl FnOnce(&Self::Type) -> StarlarkWithTypeResult,
                ) -> StarlarkWithTypeResult {
                    f(payload)
                }
            },
        ]
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
        if impl_starlark_value.attrs.unpack_value
            || impl_starlark_value.attrs.starlark_type_repr
            || impl_starlark_value.attrs.vtable != VtableMode::Default
        {
            return Err(syn::Error::new_spanned(
                &input.self_ty,
                "an `impl StarlarkType` takes only `type = ...`; unpacking and vtable \
                 registration are the representations'",
            ));
        }
        let type_impl = impl_starlark_value.type_impl(input.items.clone())?;
        let union_payload = impl_starlark_value.union_payload_is_self();
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

    if let Some(item) = input
        .items
        .iter()
        .find(|item| matches!(item, syn::ImplItem::Type(t) if t.ident == "Canonical"))
    {
        return Err(syn::Error::new_spanned(
            item,
            "there is no `Canonical`; a representation of a type with several representations \
             names the type with `type Type = ...`",
        ));
    }
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
        if impl_starlark_value.attrs.unpack_value || impl_starlark_value.attrs.starlark_type_repr {
            return Err(syn::Error::new_spanned(
                &input.self_ty,
                "one representation of a type cannot be a binding for the type; bind its `Type`",
            ));
        }
        if let Some(typ) = &impl_starlark_value.attrs.typ {
            return Err(syn::Error::new_spanned(
                typ,
                "the `type()` string is the `Type`'s; it is not given on a representation",
            ));
        }
        let with_type = if impl_starlark_value.has_fn("with_type") {
            None
        } else {
            Some(impl_starlark_value.with_type_via_from())
        };
        rep_impl.items.splice(
            0..0,
            [please_use_starlark_type_macro]
                .into_iter()
                .chain(with_type),
        );
        return Ok(quote! {
            #rep_impl

            #vtable_registration

            #alloc_impls
        });
    }

    let impl_unpack_value = impl_starlark_value.impl_unpack_value()?;
    let type_impl = impl_starlark_value.type_impl(ty_items)?;
    let type_self: syn::ImplItem = syn::parse_quote! {
        type Type = Self;
    };
    let with_type = impl_starlark_value.with_type_identity();
    rep_impl.items.splice(
        0..0,
        [please_use_starlark_type_macro, type_self]
            .into_iter()
            .chain(with_type),
    );

    Ok(quote! {
        #impl_unpack_value

        #type_impl

        #rep_impl

        #vtable_registration

        #alloc_impls
    })
}
