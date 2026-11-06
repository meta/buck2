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

use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::set::SetRef;
use crate::values::type_repr::SetType;
use crate::values::type_repr::StarlarkTypeRepr;

/// Unpack a `Set`.
pub struct UnpackSetEntries<K> {
    /// Entries of the set.
    pub entries: Vec<K>,
}

impl<K> Default for UnpackSetEntries<K> {
    fn default() -> Self {
        UnpackSetEntries {
            entries: Vec::new(),
        }
    }
}

impl<K: StarlarkTypeRepr> StarlarkTypeRepr for UnpackSetEntries<K> {
    type Canonical = <SetType<K> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> crate::typing::Ty {
        SetType::<K>::starlark_type_repr()
    }
}

impl<'v, K: UnpackValue<'v>> UnpackValue<'v> for UnpackSetEntries<K> {
    type Error = K::Error;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        let Some(set) = SetRef::unpack_value_opt(value) else {
            return Ok(None);
        };
        let mut entries = Vec::with_capacity(set.aref.content.len());
        for k in set.aref.iter() {
            let Some(k) = K::unpack_value_impl(k)? else {
                return Ok(None);
            };
            entries.push(k);
        }
        Ok(Some(UnpackSetEntries { entries }))
    }
}
