/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::Dupe;

/// Extension traits on [`Option`](Option) where it holds a ref.
pub trait OptionDupedExt {
    type Item;

    /// Like `cloned`, but with a `Dupe` constraint.
    ///
    /// ```
    /// # #[cfg(feature = "alloc")] {
    /// use std::rc::Rc;
    ///
    /// use dupe::OptionDupedExt;
    /// let rc = Rc::new("test");
    /// assert_eq!(Some(&rc).duped(), Some(rc));
    /// assert_eq!(None::<&Rc<String>>.duped(), None);
    /// # }
    /// ```
    fn duped(self) -> Option<Self::Item>
    where
        Self::Item: Dupe;
}

impl<T> OptionDupedExt for Option<&T> {
    type Item = T;

    fn duped(self) -> Option<T>
    where
        T: Dupe,
    {
        self.map(|x| x.dupe())
    }
}
