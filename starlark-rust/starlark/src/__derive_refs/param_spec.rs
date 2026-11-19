/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use dupe::Dupe;
use pagable::StaticStr;

use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::callable_param::ParamIsRequired;
use crate::typing::macro_support::unpack_args_item_ty;
use crate::typing::macro_support::unpack_kwargs_value_ty;
use crate::util::arc_str::ArcStr;
use crate::values::FrozenHeap;
use crate::values::Value;

pub enum NativeCallableParamDefaultValue {
    /// Allocates the default on the builder's heap. Used for documentation only, not when the
    /// function is called.
    Value(for<'fh> fn(FrozenHeap<'fh>) -> Value<'fh>),
    Optional,
}

pub struct NativeCallableParam {
    pub name: StaticStr,
    /// Type of the parameter.
    /// For `*args` is the type of the element, and for `**kwargs` is the type of the value.
    pub ty: Ty,
    /// `None` means the parameter is required.
    pub required: Option<NativeCallableParamDefaultValue>,
    /// Accepted by the callable but omitted from generated documentation.
    pub doc_hidden: bool,
}

impl NativeCallableParam {
    pub fn args(name: StaticStr, param_ty: Ty) -> NativeCallableParam {
        NativeCallableParam {
            name,
            ty: unpack_args_item_ty(param_ty),
            required: None,
            doc_hidden: false,
        }
    }

    pub fn kwargs(name: StaticStr, param_ty: Ty) -> NativeCallableParam {
        NativeCallableParam {
            name,
            ty: unpack_kwargs_value_ty(param_ty),
            required: None,
            doc_hidden: false,
        }
    }

    fn is_required(&self) -> ParamIsRequired {
        match self.required {
            None => ParamIsRequired::Yes,
            Some(_) => ParamIsRequired::No,
        }
    }
}

pub struct NativeCallableParamSpec {
    pub pos_only: Vec<NativeCallableParam>,
    pub pos_or_named: Vec<NativeCallableParam>,
    pub args: Option<NativeCallableParam>,
    pub named_only: Vec<NativeCallableParam>,
    pub kwargs: Option<NativeCallableParam>,
}

pagable::static_str!(ARGS_NAME = "args");
pagable::static_str!(KWARGS_NAME = "kwargs");

impl NativeCallableParamSpec {
    /// For a function accepting raw `&Arguments`.
    pub fn for_arguments() -> NativeCallableParamSpec {
        NativeCallableParamSpec {
            pos_only: Vec::new(),
            pos_or_named: Vec::new(),
            args: Some(NativeCallableParam::args(ARGS_NAME, Ty::any())),
            named_only: Vec::new(),
            kwargs: Some(NativeCallableParam::kwargs(KWARGS_NAME, Ty::any())),
        }
    }

    pub(crate) fn param_spec(&self) -> ParamSpec {
        ParamSpec::new_parts(
            self.pos_only.iter().map(|p| (p.is_required(), p.ty.dupe())),
            self.pos_or_named
                .iter()
                .map(|p| (ArcStr::new_static(p.name), p.is_required(), p.ty.dupe())),
            self.args.as_ref().map(|p| p.ty.dupe()),
            self.named_only
                .iter()
                .map(|p| (ArcStr::new_static(p.name), p.is_required(), p.ty.dupe())),
            self.kwargs.as_ref().map(|p| p.ty.dupe()),
        )
        .unwrap()
    }
}
