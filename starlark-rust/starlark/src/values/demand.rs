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

use std::any::TypeId;
use std::marker::PhantomData;

use crate::values::StarlarkTypeExpr;
use crate::values::Value;
use crate::values::predicate::StarlarkPredicate;

/// A request for the handle of one predicate type to a value. It is what the
/// `provide_predicate_handle` of the value's type is handed, and the type answers it if it is
/// a member of that predicate type.
///
/// [`#[starlark_value]`](crate::values::starlark_value) writes both the function and what it
/// does with this; there is nothing to do with one by hand.
pub struct Demand<'a, 'v> {
    /// The marker of the predicate type `P` whose handle is requested.
    predicate: TypeId,
    /// `&'a mut Option<P::Get>`.
    option: *mut (),
    _marker: PhantomData<&'a mut &'v ()>,
}

impl<'a, 'v> Demand<'a, 'v> {
    fn new<P: StarlarkPredicate>(
        option: &'a mut Option<<P as StarlarkTypeExpr<'v>>::Get>,
    ) -> Demand<'a, 'v> {
        Demand {
            predicate: TypeId::of::<P>(),
            option: option as *mut _ as *mut (),
            _marker: PhantomData,
        }
    }

    /// Provide the handle of the predicate type `P`, made by `handle`, if that is what is
    /// requested.
    #[inline]
    pub fn provide_handle<P: StarlarkPredicate>(
        &mut self,
        handle: impl FnOnce() -> <P as StarlarkTypeExpr<'v>>::Get,
    ) {
        if self.predicate == TypeId::of::<P>() {
            // SAFETY: the option is the one `new::<P>` was given.
            unsafe {
                *(self.option as *mut Option<<P as StarlarkTypeExpr<'v>>::Get>) = Some(handle())
            };
        }
    }
}

/// The handle of the predicate type `P` to `value`, if the type of `value` says in its
/// `#[starlark_value]` block that it is a member.
#[inline]
pub(crate) fn request_predicate_handle<'v, P: StarlarkPredicate>(
    value: Value<'v>,
) -> Option<<P as StarlarkTypeExpr<'v>>::Get> {
    let value = value.get_ref();
    if !value.has_predicate_handles() {
        return None;
    }
    let mut option = None;
    value.provide_predicate_handle(&mut Demand::new::<P>(&mut option));
    option
}
