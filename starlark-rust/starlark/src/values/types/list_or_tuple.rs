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

//! `list[T] | tuple[T, ...]`, the usual type of a parameter that takes a sequence.

use either::Either;

use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::list::ListRef;
use crate::values::list::ListType;
use crate::values::tuple::TupleType;
use crate::values::type_expr::StarlarkTypeExpr;

/// `list[T] | tuple[T, ...]`, the usual type of a parameter that takes a sequence of `T`.
pub type ListOrTupleType<T> = Either<ListType<T>, TupleType<T>>;

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, ListOrTupleType<T>> {
    /// The items, as [`ValueOf<ListType<T>>::iter`](ValueOf::iter) or
    /// [`ValueOf<TupleType<T>>::iter`](ValueOf::iter) gives them.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        let value = self.to_value();
        // A list and a tuple are different representations, so which side this is cannot
        // change; the items are re-checked by that side's `iter` as for any sequence.
        if ListRef::from_value(value).is_some() {
            Either::Left(ValueOf::<ListType<T>>::new_unchecked(value).iter())
        } else {
            Either::Right(ValueOf::<TupleType<T>>::new_unchecked(value).iter())
        }
    }

    /// The items, each converted by `U`, a conversion from `T`, as
    /// [`ValueOf<ListType<T>>::try_items`](ValueOf::try_items) or
    /// [`ValueOf<TupleType<T>>::try_items`](ValueOf::try_items) gives them.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        let value = self.to_value();
        if ListRef::from_value(value).is_some() {
            Either::Left(ValueOf::<ListType<T>>::new_unchecked(value).try_items::<U>())
        } else {
            Either::Right(ValueOf::<TupleType<T>>::new_unchecked(value).try_items::<U>())
        }
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.try_items().collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::Value;
    use crate::values::ValueOf;
    use crate::values::list_or_tuple::ListOrTupleType;

    #[test]
    fn test_unpack() {
        fn unpack<'v>(v: Value<'v>) -> Option<Vec<&'v str>> {
            ValueOf::<ListOrTupleType<&'v str>>::new(v).map(|v| v.try_to_vec().unwrap())
        }
        Heap::temp(|heap| {
            assert_eq!(Some(vec!["a", "b"]), unpack(heap.alloc(vec!["a", "b"])));
            assert_eq!(Some(vec!["a", "b"]), unpack(heap.alloc(("a", "b"))));
            assert_eq!(None, unpack(heap.alloc(vec![1, 2])));
            assert_eq!(None, unpack(heap.alloc((1, 2))));
            assert_eq!(None, unpack(heap.alloc(1)));
        });
    }
}
