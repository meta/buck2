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

use std::convert::Infallible;
use std::fmt;
use std::fmt::Display;
use std::iter;
use std::ops::Deref;
use std::slice;

use ref_cast::RefCastCustom;
use ref_cast::ref_cast_custom;

use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::list::value::FrozenListData;
use crate::values::list::value::ListGen;
use crate::values::list::value::display_list;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::list::value::ListData;

/// Reference to list content (mutable or frozen).
#[repr(transparent)]
#[derive(RefCastCustom)]
pub struct ListRef<'v> {
    pub(crate) content: [Value<'v>],
}

impl<'v> ListRef<'v> {
    /// `type([])`, which is `"list"`.
    pub const TYPE: &'static str = ListData::TYPE;

    #[ref_cast_custom]
    pub(crate) fn new<'a>(slice: &'a [Value<'v>]) -> &'a ListRef<'v>;

    /// Empty list reference.
    pub fn empty() -> &'v ListRef<'v> {
        ListRef::new(&[])
    }

    /// List elements.
    pub fn content(&self) -> &[Value<'v>] {
        &self.content
    }

    /// Iterate over the elements in the list.
    pub fn iter<'a>(&'a self) -> iter::Copied<slice::Iter<'a, Value<'v>>>
    where
        'v: 'a,
    {
        self.content.iter().copied()
    }

    /// Downcast the value to the list or frozen list (both are represented by `ListRef`).
    #[inline]
    pub fn from_value(x: Value<'v>) -> Option<&'v ListRef<'v>> {
        if x.is_frozen() {
            x.downcast_ref::<ListGen<FrozenListData<'v>>>()
                .map(|x| ListRef::new(x.0.content()))
        } else {
            let ptr = x.downcast_ref::<ListGen<ListData>>()?;
            Some(ListRef::new(ptr.0.content()))
        }
    }
}

impl<'v> Deref for ListRef<'v> {
    type Target = [Value<'v>];

    fn deref(&self) -> &[Value<'v>] {
        &self.content
    }
}

impl<'v> Display for ListRef<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        display_list(&self.content, f)
    }
}

impl<'v> StarlarkTypeRepr for &'v ListRef<'v> {
    type Canonical = <Vec<Value<'v>> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        Vec::<Value<'v>>::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for &'v ListRef<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(ListRef::from_value(value))
    }
}
