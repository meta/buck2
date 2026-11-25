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

/// Marks a `#[starlark_module]` as a module of methods and attributes: the function that
/// declares one takes a `&mut MethodsBuilder`.
///
/// ```ignore
/// #[starlark_module]
/// fn foo_methods(builder: &mut MethodsBuilder) {
///     fn bar(this: &Foo) -> starlark::Result<i32> { .. }
/// }
///
/// #[starlark_value(type = "foo", methods = foo_methods)]
/// impl<'v> StarlarkValue<'v> for Foo {}
/// ```
///
/// The module is a `static` table of its members, a
/// [`MemberTable`](crate::environment::MemberTable). Nothing is built, so there is no value of
/// this type and the function is never called.
pub enum MethodsBuilder {}
