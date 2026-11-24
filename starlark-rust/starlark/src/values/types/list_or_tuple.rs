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
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::list::ListOf;
use crate::values::list::ListRef;
use crate::values::list::ListType;
use crate::values::list::StarlarkList;
use crate::values::tuple::TupleOf;
use crate::values::tuple::TupleType;
use crate::values::type_expr::StarlarkTypeExpr;

/// `list[T] | tuple[T, ...]`, the usual type of a parameter that takes a sequence of `T`.
pub type ListOrTupleType<T> = Either<ListType<T>, TupleType<T>>;

/// `list[T] | tuple[T, ...]`, the usual type of a parameter that takes a sequence of `T`.
/// `StarlarkListOrTuple<'v>` is a list or a tuple of anything.
///
/// A parameter of this type is one or the other side; code that only walks the items binds
/// the handle, `ValueOf<'v, StarlarkListOrTuple<'v, T>>`, which has `iter`, `try_items` and
/// `try_to_vec` for both.
pub type StarlarkListOrTuple<'v, T = Value<'v>> = Either<StarlarkList<'v, T>, TupleType<T>>;

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, StarlarkListOrTuple<'v, T>> {
    /// The items, as [`StarlarkList::iter`] or
    /// [`ValueOf<TupleType<T>>::iter`](ValueOf::iter) gives them.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        self.list_or_tuple()
            .map_either(StarlarkList::iter, TupleOf::iter)
    }

    /// The items, each converted by `U`, a conversion from `T`, as
    /// [`StarlarkList::try_items`] or
    /// [`ValueOf<TupleType<T>>::try_items`](ValueOf::try_items) gives them.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.list_or_tuple()
            .map_either(StarlarkList::try_items::<U>, TupleOf::try_items::<U>)
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.try_items().collect()
    }

    /// The side that the value is, looked up; its items are not checked. A list and a tuple
    /// are different representations, so which side this is cannot change.
    fn list_or_tuple(self) -> Either<StarlarkList<'v, T>, TupleOf<'v, T>> {
        let value = self.to_value();
        match StarlarkList::from_value(value) {
            Some(list) => Either::Left(StarlarkList::of_repr(list.repr())),
            None => Either::Right(TupleOf::of_handle(ValueOf::new_unchecked(value))),
        }
    }
}

/// A handle under either of the two names of the type is a handle under the other, while
/// [`ListOrTupleType`] still exists.
impl<'v, T: StarlarkTypeExpr<'v>> From<ValueOf<'v, ListOrTupleType<T>>>
    for ValueOf<'v, StarlarkListOrTuple<'v, T>>
{
    #[inline]
    fn from(value: ValueOf<'v, ListOrTupleType<T>>) -> Self {
        // The same type under its other name.
        ValueOf::new_unchecked(value.to_value())
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> From<ValueOf<'v, StarlarkListOrTuple<'v, T>>>
    for ValueOf<'v, ListOrTupleType<T>>
{
    #[inline]
    fn from(value: ValueOf<'v, StarlarkListOrTuple<'v, T>>) -> Self {
        // The same type under its other name.
        ValueOf::new_unchecked(value.to_value())
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, ListOrTupleType<T>> {
    /// The items, as [`ValueOf<ListType<T>>::iter`](ValueOf::iter) or
    /// [`ValueOf<TupleType<T>>::iter`](ValueOf::iter) gives them.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        self.side().map_either(ListOf::iter, TupleOf::iter)
    }

    /// The items, each converted by `U`, a conversion from `T`, as
    /// [`ValueOf<ListType<T>>::try_items`](ValueOf::try_items) or
    /// [`ValueOf<TupleType<T>>::try_items`](ValueOf::try_items) gives them.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.side()
            .map_either(ListOf::try_items::<U>, TupleOf::try_items::<U>)
    }

    /// The side that the value is, looked up; its items are not checked. A list and a tuple
    /// are different representations, so which side this is cannot change.
    fn side(self) -> Either<ListOf<'v, T>, TupleOf<'v, T>> {
        let value = self.to_value();
        match ListRef::from_value(value) {
            Some(list) => Either::Left(ListOf::of_handle_and_list(
                ValueOf::new_unchecked(value),
                list,
            )),
            None => Either::Right(TupleOf::of_handle(ValueOf::new_unchecked(value))),
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
    use crate::values::list::AllocList;
    use crate::values::list_or_tuple::ListOrTupleType;
    use crate::values::list_or_tuple::StarlarkListOrTuple;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_unpack() {
        fn unpack<'v>(v: Value<'v>) -> Option<Vec<&'v str>> {
            let old = ValueOf::<ListOrTupleType<StarlarkStr>>::new(v)
                .map(|v| v.try_to_vec::<&str>().unwrap());
            let new = ValueOf::<StarlarkListOrTuple<StarlarkStr>>::new(v)
                .map(|v| v.try_to_vec::<&str>().unwrap());
            assert_eq!(old, new);
            new
        }
        Heap::temp(|heap| {
            assert_eq!(
                Some(vec!["a", "b"]),
                unpack(heap.alloc(AllocList(vec!["a", "b"])).to_value())
            );
            assert_eq!(
                Some(vec!["a", "b"]),
                unpack(heap.alloc(("a", "b")).to_value())
            );
            assert_eq!(
                None,
                unpack(heap.alloc(AllocList(vec![1i32, 2i32])).to_value())
            );
            assert_eq!(None, unpack(heap.alloc((1i32, 2i32)).to_value()));
            assert_eq!(None, unpack(heap.alloc(1i32).to_value()));
        });
    }
}
