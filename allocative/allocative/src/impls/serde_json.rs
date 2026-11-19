/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(feature = "serde_json")]

use std::mem;

use crate::Allocative;
use crate::Visitor;
use crate::key;

impl Allocative for serde_json::Number {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let visitor = visitor.enter_self_sized::<Self>();
        // Number may have allocations, but it is opaque.
        visitor.exit();
    }
}

impl Allocative for serde_json::Map<String, serde_json::Value> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        {
            let mut visitor = visitor.enter_unique(key!("data"), mem::size_of::<*mut ()>());
            for (k, v) in self.iter() {
                visitor.visit_field(key!("key"), k);
                visitor.visit_field(key!("value"), v);
            }
            visitor.exit();
        }
        visitor.exit();
    }
}

impl Allocative for serde_json::Value {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        match self {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(_b) => {}
            serde_json::Value::Number(n) => visitor.visit_field(key!("Number"), n),
            serde_json::Value::String(s) => visitor.visit_field(key!("String"), s),
            serde_json::Value::Array(a) => visitor.visit_field(key!("Array"), a),
            serde_json::Value::Object(o) => visitor.visit_field(key!("Object"), o),
        }
        visitor.exit();
    }
}
