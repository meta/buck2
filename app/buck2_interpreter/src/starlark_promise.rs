/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A type [`StarlarkPromise`] which provides basic promise-like functionality.
use std::cell::Cell;
use std::cell::RefCell;
use std::convert::Infallible;
use std::mem;

use allocative::Allocative;
use derivative::Derivative;
use derive_more::Display;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::typing::Ty;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::AllocList;
use starlark::values::starlark_value;
use starlark::values::tuple::TupleType;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark::values::typing::StarlarkCallable;

/// A type that corresponds to a Rust promise.
#[derive(
    ProvidesStaticType,
    NoSerialize,
    Display,
    Derivative,
    Trace,
    Allocative
)]
#[derivative(Debug)]
#[display("promise()")]
pub struct StarlarkPromise<'v> {
    /// The value of the promise.
    value: RefCell<PromiseValue<'v>>,
    /// Everyone who is downstream of this promise and needs executing when it changes.
    /// These will all be `.map` style nodes.
    /// Note that as a consequence of this back pointer, we can't garbage collect promises
    /// that are generated from this promise, even if they aren't used.
    #[derivative(Debug = "ignore")]
    downstream: RefCell<Vec<ValueTyped<'v, StarlarkPromise<'v>>>>,
    /// Things we want to validate whenever the value of this promise is resolved.
    #[derivative(Debug = "ignore")]
    validate: RefCell<Vec<Validate<'v>>>,
}

#[derive(Allocative, Trace)]
struct Validate<'v>(#[trace(unsafe_ignore)] fn(Value<'v>) -> buck2_error::Result<()>);

#[derive(Clone, Debug, Trace, Allocative)]
enum PromiseValue<'v> {
    Unresolved,
    Resolved(Value<'v>),
    Map(
        ValueTyped<'v, StarlarkPromise<'v>>,
        StarlarkCallable<'v, (Value<'v>,), Value<'v>>,
    ),
    Join(PromiseJoin<'v>),
}

#[derive(Clone, Debug, Trace, Allocative)]
struct PromiseJoin<'v> {
    /// Number of items that are currently unresolved
    #[allocative(skip)]
    unresolved: Cell<usize>,
    items: Vec<ValueTyped<'v, StarlarkPromise<'v>>>,
}

impl<'v> PromiseJoin<'v> {
    fn new(items: Vec<ValueTyped<'v, StarlarkPromise<'v>>>) -> Self {
        let unresolved = Cell::new(items.iter().filter(|x| x.get().is_none()).count());
        Self { unresolved, items }
    }

    fn resolve_one(&self) {
        self.unresolved
            .set(self.unresolved.get().checked_sub(1).unwrap());
    }

    fn get(&self) -> Option<Vec<Value<'v>>> {
        if self.unresolved.get() != 0 {
            None
        } else {
            let mut res = Vec::with_capacity(self.items.len());
            for x in &self.items {
                res.push(x.get().expect("invariant broken in promise join"))
            }
            Some(res)
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PromiseError {
    #[error("Can't .resolve on a promise produced with .map")]
    CantResolveMap,
    #[error("Can't .resolve on a promise which already has a value")]
    CantResolveTwice,
}

impl<'v> StarlarkPromise<'v> {
    /// Create a new unresolved promise.
    /// Must have [`StarlarkPromise::resolve`] called on it later.
    pub fn new_unresolved() -> Self {
        Self {
            value: RefCell::new(PromiseValue::Unresolved),
            downstream: RefCell::new(Vec::new()),
            validate: RefCell::new(Vec::new()),
        }
    }

    /// Create a new resolved promise.
    pub fn new_resolved(value: Value<'v>) -> Self {
        Self {
            value: RefCell::new(PromiseValue::Resolved(value)),
            ..Self::new_unresolved()
        }
    }

    /// Get the value from a resolved promise, or [`None`] otherwise.
    pub fn get(&self) -> Option<Value<'v>> {
        match &*self.value.borrow() {
            PromiseValue::Resolved(x) => Some(*x),
            _ => None,
        }
    }

    /// A recursive version of [`StarlarkPromise::get`], which continues to see through
    /// promises while they are resolved.
    /// The returned value will either be an unresolved promise, or not a promise.
    pub fn get_recursive(mut value: Value<'v>) -> Value<'v> {
        while let Some(promise) = StarlarkPromise::from_value(value) {
            if let Some(x) = promise.get() {
                value = x;
            } else {
                // We have an unresolved promise, stop looping
                break;
            }
        }
        value
    }

    fn apply(
        f: StarlarkCallable<'v, (Value<'v>,), Value<'v>>,
        x: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<Value<'v>> {
        Ok(eval.eval_function(f.0, &[x], &[])?)
    }

    pub fn map(
        x: ValueTyped<'v, StarlarkPromise<'v>>,
        f: StarlarkCallable<'v, (Value<'v>,), Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        match x.get() {
            Some(x) => Ok(eval
                .heap()
                .alloc_typed(Self::new_resolved(Self::apply(f, x, eval)?))),
            _ => {
                let res = eval.heap().alloc_typed(Self {
                    value: RefCell::new(PromiseValue::Map(x, f)),
                    ..Self::new_unresolved()
                });
                x.downstream.borrow_mut().push(res);
                Ok(res)
            }
        }
    }

    pub fn join(
        args: Vec<ValueTyped<'v, StarlarkPromise<'v>>>,
        heap: Heap<'v>,
    ) -> ValueTyped<'v, StarlarkPromise<'v>> {
        let join = PromiseJoin::new(args);
        match join.get() {
            Some(values) => heap.alloc_typed(Self::new_resolved(heap.alloc(AllocList(values)))),
            None => {
                let promise = Self::new_unresolved();
                let value = heap.alloc_typed(promise);
                for arg in &join.items {
                    arg.downstream.borrow_mut().push(value);
                }
                *value.value.borrow_mut() = PromiseValue::Join(join);
                value
            }
        }
    }

    /// Validate the type of a promise. Will execute once the promise is resolved.
    pub fn validate(&self, f: fn(Value<'v>) -> buck2_error::Result<()>) -> buck2_error::Result<()> {
        match self.get() {
            Some(x) => f(x),
            _ => {
                self.validate.borrow_mut().push(Validate(f));
                Ok(())
            }
        }
    }

    /// Resolve a promise. Errors if the promise was produced by `.map` or the promise has
    /// already been resolved.
    pub fn resolve(
        &self,
        x: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<()> {
        if matches!(&*self.value.borrow(), PromiseValue::Map(..)) {
            return Err(PromiseError::CantResolveMap.into());
        }
        self.resolve_rec(x, eval)
    }

    fn resolve_rec(
        &self,
        x: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<()> {
        if matches!(&*self.value.borrow(), PromiseValue::Resolved(_)) {
            return Err(PromiseError::CantResolveTwice.into());
        }
        for f in self.validate.borrow().iter() {
            f.0(x)?;
        }
        *self.value.borrow_mut() = PromiseValue::Resolved(x);
        // We clear validate/downstream because we don't need them anymore, so might as well save the memory
        self.validate.borrow_mut().clear();
        let mut downstream = Vec::new();
        mem::swap(&mut *self.downstream.borrow_mut(), &mut downstream);
        for d in downstream {
            // Make sure we drop the borrow BEFORE calling resolve_rec
            let borrow = d.value.borrow();
            match &*borrow {
                PromiseValue::Map(_, f) => {
                    let f = *f;
                    drop(borrow);
                    d.resolve_rec(Self::apply(f, x, eval)?, eval)?;
                }
                PromiseValue::Join(join) => {
                    join.resolve_one();
                    if let Some(elems) = join.get() {
                        drop(borrow);
                        d.resolve_rec(eval.heap().alloc(AllocList(elems)), eval)?;
                    }
                }
                _ => panic!("Impossible to reach a downstream promise that is not a map or join"),
            }
        }
        Ok(())
    }

    /// Downcast the value.
    pub fn from_value(x: Value<'v>) -> Option<&'v Self> {
        x.downcast_ref()
    }
}

// We can't use `starlark_complex_value!` because there is no frozen form of a promise

impl<'v> AllocValue<'v> for StarlarkPromise<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        // FIXME: need to be able to freeze things that are resolved
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkTypeRepr for &'v StarlarkPromise<'v> {
    type Canonical = Self;

    fn starlark_type_repr() -> Ty {
        StarlarkPromise::get_type_starlark_repr()
    }
}

impl<'v> UnpackValue<'v> for &'v StarlarkPromise<'v> {
    type Error = Infallible;

    fn unpack_value_impl(x: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(StarlarkPromise::from_value(x))
    }
}

starlark::methods_static!(PROMISE_METHODS = promise_methods);

#[starlark_value(type = "Promise")]
impl<'v> StarlarkValue<'v> for StarlarkPromise<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(PROMISE_METHODS.methods())
    }
}

#[starlark_module]
fn promise_methods(builder: &mut MethodsBuilder) {
    /// Given a promise, apply a function to the value it contains, producing a promise with the resulting value.
    fn map<'v>(
        this: ValueTyped<'v, StarlarkPromise<'v>>,
        #[starlark(require = pos)] func: StarlarkCallable<'v, (Value<'v>,), Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        Ok(StarlarkPromise::map(this, func, eval)?)
    }

    /// Join a set of promises together into a single promise.
    ///
    /// Given a series of promises, `p4 = p1.join(p2, p3)` will produce a promise
    /// where the value is promise that resolves to a tuple containing the three values,
    /// those from `p1`, `p2` and `p3` respectively.
    fn join<'v>(
        this: ValueTyped<'v, StarlarkPromise<'v>>,
        #[starlark(args)] args: ValueOf<'v, TupleType<ValueTyped<'v, StarlarkPromise<'v>>>>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        let args = std::iter::once(this).chain(args.items()).collect();
        Ok(StarlarkPromise::join(args, heap))
    }
}

#[starlark_module]
#[starlark_types(StarlarkPromise<'_> as Promise)]
pub fn register_promise(globals: &mut GlobalsBuilder) {}

#[cfg(test)]
mod tests {

    use buck2_error::buck2_error;
    use starlark::any::ProvidesStaticType;
    use starlark::environment::Module;
    use starlark::syntax::AstModule;
    use starlark::values::none::NoneType;
    use starlark::values::tuple::TupleRef;

    use super::*;
    use crate::file_type::StarlarkFileType;

    #[derive(
        ProvidesStaticType,
        NoSerialize,
        Trace,
        Default,
        Debug,
        Display,
        Allocative
    )]
    #[display("{:?}", self)]
    struct Promises<'v>(RefCell<Vec<(String, ValueTyped<'v, StarlarkPromise<'v>>)>>);

    #[starlark_value(type = "Promises")]
    impl<'v> StarlarkValue<'v> for Promises<'v> {}

    #[starlark_module]
    fn helpers(builder: &mut GlobalsBuilder) {
        fn promise_unresolved<'v>(
            name: String,
            eval: &mut Evaluator<'v, '_, '_>,
        ) -> starlark::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
            let promises = get_promises(eval.module());
            let promise = eval.heap().alloc_typed(StarlarkPromise::new_unresolved());
            promises.0.borrow_mut().push((name, promise));
            Ok(promise)
        }

        fn promise_resolved<'v>(value: Value<'v>) -> starlark::Result<StarlarkPromise<'v>> {
            Ok(StarlarkPromise::new_resolved(value))
        }

        fn promise_validate<'v>(
            promise: ValueTyped<'v, StarlarkPromise<'v>>,
        ) -> starlark::Result<NoneType> {
            promise.validate(|x| {
                if x.unpack_str() == Some("ok") {
                    Ok(())
                } else {
                    Err(buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "VALIDATE_FAILED"
                    ))
                }
            })?;
            Ok(NoneType)
        }
    }

    fn alloc_promises(modu: &Module) {
        modu.set(
            "__promises__",
            modu.heap().alloc_complex_no_freeze(Promises::default()),
        )
    }

    fn get_promises<'v>(modu: &Module<'v>) -> &'v Promises<'v> {
        modu.get("__promises__").unwrap().downcast_ref().unwrap()
    }

    fn assert_promise<'v>(modu: &Module<'v>, content: &str) -> buck2_error::Result<Value<'v>> {
        alloc_promises(modu);
        let globals = GlobalsBuilder::standard().with(helpers).build();
        let ast = AstModule::parse(
            "test.bzl",
            content.to_owned(),
            &StarlarkFileType::Bzl.dialect(false),
        )?;
        let mut eval = Evaluator::new(modu);
        let res = eval.eval_module(ast, &globals)?;
        let promises = get_promises(modu);
        for (key, promise) in promises.0.borrow().iter() {
            promise.resolve(modu.heap().alloc(key), &mut eval)?;
        }
        Ok(res)
    }

    #[allow(clippy::needless_lifetimes)]
    fn assert_promise_err<'v>(modu: &Module<'v>, content: &str, err: &str) -> buck2_error::Error {
        match assert_promise(modu, content) {
            Ok(_) => panic!("Expected an error, got a result"),
            Err(e) => {
                if format!("{e:#}").contains(err) {
                    e
                } else {
                    panic!("Wrong error message, expected to see `{err}`, got `{e:?}`")
                }
            }
        }
    }

    #[test]
    fn test_promise() {
        // Test
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|modu| {
            let res = assert_promise(
                &modu,
                r#"
a = promise_unresolved("test")
b = a.map(lambda x: x.upper())
c = b.map(lambda x: x + "!")
d = a.map(lambda x: x.title())
e = promise_resolved("more")
f = e.map(lambda x: x.upper())
(a,b,c,d,e,f)
"#,
            )
            .unwrap();
            let wants = &["test", "TEST", "TEST!", "Test", "more", "MORE"];
            for (want, got) in wants
                .iter()
                .zip(TupleRef::from_value(res).unwrap().content())
            {
                assert_eq!(
                    StarlarkPromise::from_value(*got)
                        .unwrap()
                        .get()
                        .unwrap()
                        .unpack_str()
                        .unwrap(),
                    *want
                );
            }
            starlark::Result::Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_promise_validate() {
        // Test
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|modu| {
            assert_promise(
                &modu,
                r#"
p = promise_unresolved("ok")
promise_validate(p)
p
"#,
            )
            .unwrap();
            assert_promise_err(
                &modu,
                r#"
p = promise_unresolved("test")
promise_validate(p)
p
"#,
                "VALIDATE_FAILED",
            );
            starlark::Result::Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_promise_join() {
        // Test
        // ast-grep-ignore: rust/buck2-no-starlark-module
        Module::with_temp_heap(|modu| {
            let res = assert_promise(
                &modu,
                r#"
p1 = promise_resolved("a")
p2 = promise_resolved("b")
p3 = promise_resolved("c")
p1.join(p2, p3)
"#,
            )
            .unwrap();
            assert_eq!(
                StarlarkPromise::from_value(res)
                    .unwrap()
                    .get()
                    .unwrap()
                    .to_string(),
                "[\"a\", \"b\", \"c\"]"
            );
            let res = assert_promise(
                &modu,
                r#"
p1 = promise_resolved("a")
p2 = promise_unresolved("B")
p3 = promise_unresolved("C")
p1.join(p2, p3)
"#,
            )
            .unwrap();
            assert_eq!(
                StarlarkPromise::from_value(res)
                    .unwrap()
                    .get()
                    .unwrap()
                    .to_string(),
                "[\"a\", \"B\", \"C\"]"
            );
            starlark::Result::Ok(())
        })
        .unwrap();
    }
}
