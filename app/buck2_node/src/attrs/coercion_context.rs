/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::cell::RefMut;

use allocative::Allocative;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckMutMap;
use buck2_query::query::syntax::simple::functions::QueryLiteralVisitor;
use buck2_query_parser::Expr;
use buck2_query_parser::spanned::Spanned;
use buck2_util::arc_str::ArcStr;
use buck2_util::strong_hasher::StrongHash128;
use starlark::any::ProvidesStaticType;
use starlark::values::Trace;
use starlark::values::Tracer;
use starlark::values::Value;
use starlark::values::ValueIdentity;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::attr_type::AttrType;
use crate::attrs::coerced_path::CoercedPath;
use crate::attrs::configurable::AttrIsConfigurable;
use crate::modifiers::CfgModifiers;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum AttrCoercionContextError {
    #[error("Expected target label without name. Got `{0}`")]
    UnexpectedProvidersName(String),
}

/// Memo key identifying one coercion of a frozen starlark value.
///
/// The coercion result is not a function of the input value alone: the same
/// value can coerce to different results — or fail for one attribute and
/// succeed for another — so the key carries every input the result depends
/// on. Besides the value itself (identified by `ValueIdentity<'v>`, whose
/// soundness starlark owns), every component is an identity-grade content
/// hash, so keys stay valid regardless of what outlives what.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Allocative)]
pub struct CoerceMemoKey<'v> {
    /// The input, by identity. Only frozen values are keyed: they are
    /// immutable and pointer-stable for the lifetime of the evaluation.
    value: ValueIdentity<'v>,
    /// The coercer, by content fingerprint: `attr.string()` and `attr.int()`
    /// coerce the same input to different results (or reject it), while
    /// structurally equal types coerce identically and may share entries.
    coercer: StrongHash128,
    /// The attribute default, by stored root content hash (`None` when the
    /// attribute has no default): selector coercion resolves entries against
    /// it, so the same input under a different default can produce a
    /// different result.
    default: Option<StrongHash128>,
    /// Whether `select()` is permitted: affects both the result shape and
    /// whether coercion succeeds at all.
    configurable: bool,
}

impl<'v> CoerceMemoKey<'v> {
    /// `None` for non-frozen values (mutable, so not memoizable) and for
    /// defaults that are not arena roots (no stored identity hash to key by).
    pub fn new(
        value: Value<'v>,
        coercer: &AttrType,
        default_attr: Option<CoercedAttrRef<'_>>,
        configurable: AttrIsConfigurable,
    ) -> Option<Self> {
        if !value.is_frozen() {
            return None;
        }
        let default = match default_attr {
            None => None,
            Some(d) => Some(d.root_strong_hash()?),
        };
        Some(CoerceMemoKey {
            value: value.identity(),
            coercer: coercer.fingerprint(),
            default,
            configurable: configurable == AttrIsConfigurable::Yes,
        })
    }
}

/// Identity-keyed coercion results for one module evaluation. Allocated in
/// the module's extra value so `'v` is the heap lifetime that retains every
/// keyed pointer: the table provably cannot outlive the values it identifies.
#[derive(Default, Debug, Allocative)]
pub struct CoerceMemoTable<'v>(RefCell<BuckMutMap<CoerceMemoKey<'v>, AttrToken>>);

impl<'v> CoerceMemoTable<'v> {
    pub fn lookup(&self, key: CoerceMemoKey<'v>) -> Option<AttrToken> {
        self.0.borrow().get(&key).copied()
    }

    pub fn insert(&self, key: CoerceMemoKey<'v>, value: AttrToken) {
        self.0.borrow_mut().insert(key, value);
    }
}

unsafe impl<'v> ProvidesStaticType<'v> for CoerceMemoTable<'v> {
    type StaticType = CoerceMemoTable<'static>;
}

// SAFETY: keys are identities of frozen values, which the GC neither owns
// nor moves, and an arena token holds no starlark values.
unsafe impl<'v> Trace<'v> for CoerceMemoTable<'v> {
    fn trace(&mut self, _tracer: &Tracer<'v>) {}
}

/// The context for attribute coercion. Mostly just contains information about
/// the current package (to support things like parsing targets from strings).
pub trait AttrCoercionContext<'v> {
    fn coerce_target_label(&self, value: &str) -> buck2_error::Result<TargetLabel> {
        let label = self.coerce_providers_label(value)?;

        if let ProvidersName::NonDefault(_) = label.name() {
            return Err(AttrCoercionContextError::UnexpectedProvidersName(value.to_owned()).into());
        }

        Ok(label.into_parts().0)
    }

    /// Attempt to convert a string into a label
    fn coerce_providers_label(&self, value: &str) -> buck2_error::Result<ProvidersLabel>;

    /// Memo hooks for the package-arena pipeline: cache coercion results for
    /// frozen starlark values (see `CoerceMemoKey`). Contexts without a memo
    /// return `None` / ignore inserts.
    fn coerce_memo_lookup(&self, _key: CoerceMemoKey<'v>) -> Option<AttrToken> {
        None
    }

    fn coerce_memo_insert(&self, _key: CoerceMemoKey<'v>, _value: AttrToken) {}

    /// The arena builder every value coerced in this context is encoded
    /// into. It is borrowed for one `encode_*` call at a time, so a value's
    /// children are encoded before the value.
    fn attr_arena(&self) -> RefMut<'_, CoercedAttrArenaBuilder>;

    /// Parses a `modifiers` attribute value.
    fn coerce_cfg_modifiers(&self, value: Value<'v>) -> CfgModifiers;

    /// Reuse previously allocated string if possible.
    fn intern_str(&self, value: &str) -> ArcStr;

    /// Attempt to convert a string into a BuckPath
    fn coerce_path(&self, value: &str, allow_directory: bool) -> buck2_error::Result<CoercedPath>;

    fn coerce_target_pattern(
        &self,
        pattern: &str,
    ) -> buck2_error::Result<ParsedPattern<TargetPatternExtra>>;

    fn visit_query_function_literals<'q>(
        &self,
        visitor: &mut dyn QueryLiteralVisitor<'q>,
        expr: &Spanned<Expr<'q>>,
        query: &'q str,
    ) -> buck2_error::Result<()>;
}
