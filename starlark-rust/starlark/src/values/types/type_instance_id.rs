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

use std::hash::Hasher;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use starlark_derive::Freeze;
use starlark_derive::StarlarkPagableViaPagable;
use starlark_syntax::internal_error;
use strong_hash::StrongHash;

use crate as starlark;
use crate::pagable::heap_ref_id::Blake3StrongHasher;

/// Globally unique identifier for a nominal type (e.g. a `record` or `enum`).
///
/// The id is the sole discriminator for type identity — runtime matching and
/// `TyUser` equality compare ids and nothing else — so it must be:
/// - deterministic across processes (re-evaluating the same source, or a
///   page-out/page-in round trip, reconstructs the same id — this is what lets
///   content-addressed page-out blobs dedup), and
/// - unique across distinct types (so matching never false-positives).
///
/// Build ids with [`TypeInstanceId::from_identity`] from a stable identity (a
/// call site, a provider id, ...), never from process-local state.
#[derive(
    Debug,
    Copy,
    Clone,
    Dupe,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    Freeze,
    Pagable,
    StarlarkPagableViaPagable,
    StrongHash
)]
// 64-bit truncation of the identity hash. The population is distinct type
// *definitions* (source-bounded, not values), so 64 bits is collision-free in
// practice.
pub struct TypeInstanceId(pub(crate) u64);

/// Domain separation for [`TypeInstanceId::from_identity`]: a stable, globally
/// unique tag for the *kind* of nominal type, mixed into the id so two kinds
/// whose identity bytes coincide still get distinct ids.
///
/// Embedders implement this for their own kinds (e.g. buck2 providers); each
/// [`tag`](Self::tag) must be namespaced (e.g. `"buck2.provider"`) so domains
/// from different crates cannot collide.
pub trait TypeIdDomain {
    /// A stable, globally unique, namespaced discriminator for this kind.
    fn tag(&self) -> &'static str;
}

/// [`TypeIdDomain`]s for starlark's own built-in nominal types.
#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq)]
pub enum StarlarkTypeIdDomain {
    /// A `record(...)` type.
    Record,
    /// An `enum(...)` type.
    Enum,
    /// The type-of-the-type `record[X]` (whose sole instance is the record type
    /// `X`). Has no identity of its own: `X`'s id is passed as the identity.
    RecordTypeOfType,
    /// The type-of-the-type `enum[X]` (whose sole instance is the enum type
    /// `X`). Has no identity of its own: `X`'s id is passed as the identity.
    EnumTypeOfType,
}

impl TypeIdDomain for StarlarkTypeIdDomain {
    fn tag(&self) -> &'static str {
        match self {
            StarlarkTypeIdDomain::Record => "starlark.record",
            StarlarkTypeIdDomain::Enum => "starlark.enum",
            StarlarkTypeIdDomain::RecordTypeOfType => "starlark.record_type_of_type",
            StarlarkTypeIdDomain::EnumTypeOfType => "starlark.enum_type_of_type",
        }
    }
}

#[cfg(test)]
struct TestTypeIdDomain;

#[cfg(test)]
impl TypeIdDomain for TestTypeIdDomain {
    fn tag(&self) -> &'static str {
        "starlark.test_type_instance_id"
    }
}

impl TypeInstanceId {
    /// Construct a content-deterministic id from a domain tag and a stable
    /// identity.
    ///
    /// Equal `(domain, identity)` produce equal ids — i.e. the same nominal
    /// type. The `identity` must include everything that distinguishes the type
    /// from other types and nothing that varies run to run.
    pub fn from_identity(domain: impl TypeIdDomain, identity: &impl StrongHash) -> TypeInstanceId {
        let mut hasher = Blake3StrongHasher::new();
        let tag = domain.tag();
        hasher.write_usize(tag.len());
        hasher.write(tag.as_bytes());
        identity.strong_hash(&mut hasher);
        TypeInstanceId(hasher.finish())
    }

    /// Content-deterministic id for a type defined at the current `record(...)` /
    /// `enum(...)` call site (its filename + byte span).
    ///
    /// Requires a call-site location on the stack — the caller must be a builtin
    /// invoked from Starlark source. Errors when there is none.
    pub(crate) fn from_def_site(
        domain: impl TypeIdDomain,
        eval: &crate::eval::Evaluator<'_, '_, '_>,
    ) -> crate::Result<TypeInstanceId> {
        match eval.call_stack_top_location() {
            Some(loc) => Ok(Self::from_identity(
                domain,
                &(loc.filename(), loc.span.begin().get(), loc.span.end().get()),
            )),
            None => Err(internal_error!(
                "`TypeInstanceId::from_def_site` requires a Starlark call-site location to derive a deterministic type id, but the call stack has none"
            )),
        }
    }

    /// Deterministic test-only id. Distinct logical types must use distinct stable identities.
    #[cfg(test)]
    pub(crate) fn for_test(identity: &impl StrongHash) -> TypeInstanceId {
        Self::from_identity(TestTypeIdDomain, identity)
    }
}
