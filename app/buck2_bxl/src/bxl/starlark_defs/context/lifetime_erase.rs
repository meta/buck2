/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;

/// Abstracts a concrete type with one lifetime parameter behind a GAT
pub(crate) trait LifetimeErasedTypeClass: Sized {
    type Concrete<'b>;
}

/// A type that provides a mechanism to erase a lifetime parameter on a type.
///
/// Consider the case where you have a `&'a mut MyThing<'b>`. This type allows you to store it as a
/// `LifetimeErased<'a, MyThingTypeClass>`, and then get access to it in closures that look like
/// `for<'b> FnMut(&'a MyThing<'b>) -> R`.
///
/// In practice, we use this to allow storing a `&'a mut DiceComputations<'d>` in the extra value on
/// an evaluator, which is an `AnyLifetime<'e>` with only one lifetime parameter.
///
/// This behavior can also be achieved by dyn-traits, ie by upcasting a `Box<&'a MyThing<'b>>` into
/// a `Box<MyThingLike<'a> + 'a>`, but that comes with a bunch of ergonomic hits that this avoids.
pub(crate) struct LifetimeErased<'a, C: LifetimeErasedTypeClass>(
    // Note: It'd be nice to not embed the expectation that this is a `&'a mut` here; unfortunately,
    // the compiler ends up having trouble with type inference if you try and do that.
    //
    // It's important that this be a `Concrete<'a>`, not a `Concrete<'static>`, as that causes the
    // compiler to treat this conservatively for the purpose of dropck.
    &'a mut C::Concrete<'a>,
    // No auto traits. Someone could add (appropriately bounded) manual impls of whatever if they
    // like
    PhantomData<dyn Nothing>,
);

trait Nothing {}

impl<'a, C: LifetimeErasedTypeClass> LifetimeErased<'a, C> {
    pub(crate) fn new<'b: 'a>(value: &'a mut C::Concrete<'b>) -> Self {
        // SAFETY:
        // - First of all, lifetime extension: the requirement that `'b: 'a` means that this does
        //   not extend the effective lifetime of `value`, and so for example dropping the concrete
        //   value at the end of `'a` is sound
        // - Second, the only place this is used is below where it is passed to the user provided
        //   closures; at those closures though, to the user this is not `'static` but rather `'b`
        //   for some unknown `'b`. The possible values of the `'b` include the one the user
        //   provided here (as the only thing that the user can prove about `'b` is that `'b: 'a`,
        //   which we provide for here), and so that's ok too
        let v_static = unsafe {
            std::mem::transmute::<&'a mut C::Concrete<'b>, &'a mut C::Concrete<'a>>(value)
        };

        Self(v_static, PhantomData)
    }

    pub(crate) fn access<'s, R>(&'s self, f: impl for<'b> FnOnce(&'s C::Concrete<'b>) -> R) -> R {
        f(self.0)
    }

    pub(crate) fn access_mut<'s, R>(
        &'s mut self,
        f: impl for<'b> FnOnce(&'s mut C::Concrete<'b>) -> R,
    ) -> R {
        f(self.0)
    }
}
