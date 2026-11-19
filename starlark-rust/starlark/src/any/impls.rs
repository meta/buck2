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

use std::cell::Cell;
use std::cell::RefCell;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use either::Either;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;

macro_rules! provides_static_type_static {
    ( $t:ty ) => {
        unsafe impl<'a> $crate::any::ProvidesStaticType<'a> for $t {
            type StaticType = $t;
        }

        impl $crate::any::IsStaticType for $t {
            type Reinfect<'lt> = $t;
        }
    };
}

// One of the disadvantages of AnyLifetime is there is no finite covering set of
// types so we predeclare instances for things that seem useful, but the list is
// pretty adhoc
provides_static_type_static!(());
provides_static_type_static!(bool);
provides_static_type_static!(u8);
provides_static_type_static!(u16);
provides_static_type_static!(u32);
provides_static_type_static!(u64);
provides_static_type_static!(u128);
provides_static_type_static!(usize);
provides_static_type_static!(i8);
provides_static_type_static!(i16);
provides_static_type_static!(i32);
provides_static_type_static!(i64);
provides_static_type_static!(i128);
provides_static_type_static!(isize);
provides_static_type_static!(f32);
provides_static_type_static!(f64);
provides_static_type_static!(String);
provides_static_type_static!(str);

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for &'a T {
    type StaticType = &'static T::StaticType;
}
impl<T: IsStaticType + ?Sized> IsStaticType for &'static T {
    type Reinfect<'lt> = &'lt T::Reinfect<'lt>;
}

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for &'a mut T {
    type StaticType = &'static mut T::StaticType;
}
impl<T: IsStaticType + ?Sized> IsStaticType for &'static mut T {
    type Reinfect<'lt> = &'lt mut T::Reinfect<'lt>;
}

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for *const T {
    type StaticType = *const T::StaticType;
}
impl<T: IsStaticType + ?Sized> IsStaticType for *const T {
    type Reinfect<'lt> = *const T::Reinfect<'lt>;
}

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for *mut T {
    type StaticType = *mut T::StaticType;
}
impl<T: IsStaticType + ?Sized> IsStaticType for *mut T {
    type Reinfect<'lt> = *mut T::Reinfect<'lt>;
}

unsafe impl<'a, T> ProvidesStaticType<'a> for [T]
where
    T: ProvidesStaticType<'a>,
    T::StaticType: Sized,
{
    type StaticType = [T::StaticType];
}
impl<T: IsStaticType> IsStaticType for [T]
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = [T::Reinfect<'lt>];
}

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for Box<T> {
    type StaticType = Box<T::StaticType>;
}
impl<T: IsStaticType + ?Sized> IsStaticType for Box<T> {
    type Reinfect<'lt> = Box<T::Reinfect<'lt>>;
}

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for Rc<T> {
    type StaticType = Rc<T::StaticType>;
}
impl<T: IsStaticType + ?Sized> IsStaticType for Rc<T> {
    type Reinfect<'lt> = Rc<T::Reinfect<'lt>>;
}

unsafe impl<'a, T: ProvidesStaticType<'a> + ?Sized> ProvidesStaticType<'a> for Arc<T> {
    type StaticType = Arc<T::StaticType>;
}
impl<T: IsStaticType + ?Sized> IsStaticType for Arc<T> {
    type Reinfect<'lt> = Arc<T::Reinfect<'lt>>;
}

unsafe impl<'a, T: ProvidesStaticType<'a>> ProvidesStaticType<'a> for Cell<T> {
    type StaticType = Cell<T::StaticType>;
}
impl<T: IsStaticType> IsStaticType for Cell<T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = Cell<T::Reinfect<'lt>>;
}

unsafe impl<'a, T: ProvidesStaticType<'a>> ProvidesStaticType<'a> for UnsafeCell<T> {
    type StaticType = UnsafeCell<T::StaticType>;
}
impl<T: IsStaticType> IsStaticType for UnsafeCell<T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = UnsafeCell<T::Reinfect<'lt>>;
}

unsafe impl<'a, T: ProvidesStaticType<'a>> ProvidesStaticType<'a> for RefCell<T> {
    type StaticType = RefCell<T::StaticType>;
}
impl<T: IsStaticType> IsStaticType for RefCell<T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = RefCell<T::Reinfect<'lt>>;
}

unsafe impl<'a, T> ProvidesStaticType<'a> for Option<T>
where
    T: ProvidesStaticType<'a>,
    T::StaticType: Sized,
{
    type StaticType = Option<T::StaticType>;
}
impl<T: IsStaticType> IsStaticType for Option<T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = Option<T::Reinfect<'lt>>;
}

unsafe impl<'a, T, E> ProvidesStaticType<'a> for Result<T, E>
where
    T: ProvidesStaticType<'a>,
    T::StaticType: Sized,
    E: ProvidesStaticType<'a>,
    E::StaticType: Sized,
{
    type StaticType = Result<T::StaticType, E::StaticType>;
}
impl<T: IsStaticType, E: IsStaticType> IsStaticType for Result<T, E>
where
    for<'lt> T::Reinfect<'lt>: Sized,
    for<'lt> E::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = Result<T::Reinfect<'lt>, E::Reinfect<'lt>>;
}

unsafe impl<'a, L, R> ProvidesStaticType<'a> for Either<L, R>
where
    L: ProvidesStaticType<'a>,
    L::StaticType: Sized,
    R: ProvidesStaticType<'a>,
    R::StaticType: Sized,
{
    type StaticType = Either<L::StaticType, R::StaticType>;
}
impl<L: IsStaticType, R: IsStaticType> IsStaticType for Either<L, R>
where
    for<'lt> L::Reinfect<'lt>: Sized,
    for<'lt> R::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = Either<L::Reinfect<'lt>, R::Reinfect<'lt>>;
}

unsafe impl<'a, T> ProvidesStaticType<'a> for Vec<T>
where
    T: ProvidesStaticType<'a>,
    T::StaticType: Sized,
{
    type StaticType = Vec<T::StaticType>;
}
impl<T: IsStaticType> IsStaticType for Vec<T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = Vec<T::Reinfect<'lt>>;
}

unsafe impl<'a, K, V> ProvidesStaticType<'a> for HashMap<K, V>
where
    K: ProvidesStaticType<'a>,
    K::StaticType: Sized,
    V: ProvidesStaticType<'a>,
    V::StaticType: Sized,
{
    type StaticType = HashMap<K::StaticType, V::StaticType>;
}
impl<K: IsStaticType, V: IsStaticType> IsStaticType for HashMap<K, V>
where
    for<'lt> K::Reinfect<'lt>: Sized,
    for<'lt> V::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = HashMap<K::Reinfect<'lt>, V::Reinfect<'lt>>;
}

unsafe impl<'a, K, V> ProvidesStaticType<'a> for BTreeMap<K, V>
where
    K: ProvidesStaticType<'a>,
    K::StaticType: Sized,
    V: ProvidesStaticType<'a>,
    V::StaticType: Sized,
{
    type StaticType = BTreeMap<K::StaticType, V::StaticType>;
}
impl<K: IsStaticType, V: IsStaticType> IsStaticType for BTreeMap<K, V>
where
    for<'lt> K::Reinfect<'lt>: Sized,
    for<'lt> V::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = BTreeMap<K::Reinfect<'lt>, V::Reinfect<'lt>>;
}

unsafe impl<'a, A, B> ProvidesStaticType<'a> for (A, B)
where
    A: ProvidesStaticType<'a>,
    B: ProvidesStaticType<'a>,
    A::StaticType: Sized,
    B::StaticType: Sized,
{
    type StaticType = (A::StaticType, B::StaticType);
}

impl<A: IsStaticType, B: IsStaticType> IsStaticType for (A, B)
where
    for<'lt> A::Reinfect<'lt>: Sized,
    for<'lt> B::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = (A::Reinfect<'lt>, B::Reinfect<'lt>);
}
