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

use std::fmt::Display;
use std::hash::Hash;
use std::ops::Deref;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use pagable::PagableBoxDeserialize;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use pagable::StaticValue;
use pagable::static_value::HasStaticValueRegistry;

#[derive(Debug, Allocative)]
enum Inner<T: ?Sized + 'static> {
    Arc(Arc<T>),
    Static(StaticValue<T>),
}

#[derive(Debug, Allocative)]
pub(crate) struct ArcOrStatic<T: ?Sized + 'static>(Inner<T>);

impl<T: ?Sized + 'static> ArcOrStatic<T> {
    pub(crate) fn new_static(a: StaticValue<T>) -> Self {
        ArcOrStatic(Inner::Static(a))
    }

    pub(crate) fn new_arc(a: Arc<T>) -> Self {
        ArcOrStatic(Inner::Arc(a))
    }
}

impl<T: ?Sized + 'static> Deref for ArcOrStatic<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Inner::Arc(a) => a,
            Inner::Static(sv) => sv,
        }
    }
}

impl<T: Display + 'static> Display for ArcOrStatic<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + 'static> Clone for ArcOrStatic<T> {
    fn clone(&self) -> Self {
        Self(match &self.0 {
            Inner::Arc(a) => Inner::Arc(a.dupe()),
            Inner::Static(sv) => Inner::Static(*sv),
        })
    }
}

impl<T: ?Sized + 'static> Dupe for ArcOrStatic<T> {}

impl<T: ?Sized + 'static> PartialEq for ArcOrStatic<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: ?Sized + 'static> Eq for ArcOrStatic<T> where T: Eq {}

impl<T: ?Sized + 'static> PartialOrd for ArcOrStatic<T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized + 'static> Ord for ArcOrStatic<T>
where
    T: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized + 'static> Hash for ArcOrStatic<T>
where
    T: Hash,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

// Pagable round-trip: Static variant preserves via index (zero allocation on
// deserialize). Arc variant goes through `serialize_arc` so Arc identity is
// deduplicated across the page.
impl<T> PagableSerialize for ArcOrStatic<T>
where
    T: ?Sized
        + HasStaticValueRegistry
        + PagableSerialize
        + for<'de> PagableBoxDeserialize<'de>
        + Send
        + Sync
        + 'static,
{
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        match &self.0 {
            Inner::Static(sv) => {
                0u8.pagable_serialize(serializer)?;
                sv.pagable_serialize(serializer)
            }
            Inner::Arc(a) => {
                1u8.pagable_serialize(serializer)?;
                serializer.serialize_arc(a)
            }
        }
    }
}

impl<'de, T> PagableDeserialize<'de> for ArcOrStatic<T>
where
    T: ?Sized
        + HasStaticValueRegistry
        + PagableSerialize
        + for<'d> PagableBoxDeserialize<'d>
        + Send
        + Sync
        + 'static,
{
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let tag = u8::pagable_deserialize(deserializer)?;
        match tag {
            0 => {
                let sv = StaticValue::<T>::pagable_deserialize(deserializer)?;
                Ok(ArcOrStatic::new_static(sv))
            }
            1 => {
                let a = Arc::<T>::pagable_deserialize(deserializer)?;
                Ok(ArcOrStatic::new_arc(a))
            }
            _ => panic!("invalid ArcOrStatic<T> tag: {tag}"),
        }
    }
}
