/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(try_blocks)]

#[allow(unused_extern_crates)] // proc_macro is very special
extern crate proc_macro;

mod codegen;
mod parse;

/// Generates a QueryFunctions and HasModuleDescription impls for a type.
///
/// The HasModuleDescriptions will be based on the rustdoc comments on the
/// module and functions (and on the extract function names/types).
///
/// Example usage:
///
/// ```ignore
/// struct MyFunctionsContext {
///   <fill with any data>
/// }
///
/// // The query_module attribute must be passed the QueryEnvironment type to be used, it can be a type parameter
/// // as in this example or can be a concrete type.
/// #[query_module(Env)]
/// impl <Env: QueryEnvironment + OtherTrait> MyFunctionsContext {
///     // functions can be either async or not.
///     async fn some_func(
///         // functions must take a &self receiver
///         &self,
///         // the first argument can optionally be a &Env or &QueryEvaluator.
///         env: &Env,
///         // remaining arguments must all be types that implement QueryFunctionArg
///         roots: TargetSet<Env::Target>
///     ) -> Result<QueryValue<Env>, QueryError> {
///         <...>
///     }
///
///     // binary_ops can also be implemented, the binary_op type needs to be passed in an attribute
///     #[binary_op(BinaryOp::Intersect)]
///     fn intersect(&self, env: &Env, left: TargetSet<Env::Target>, right: TargetSet<Env::Target>) -> Result<TargetSet<Env::Target>, QueryError> {
///         <...>
///     }
/// }
/// ```
///
/// Each function and binary op will be available on the QueryFunctions impl, each one uses a
/// unique opaque struct that implements QueryFunction/BinaryOp. For the `some_func` in the example above, one
/// of those structs looks like:
///
/// ```ignore
/// #[derive(RefCast)]
/// #[repr(transparent)]
/// struct __OpaqueSomeFunc(MyFunctionsContext);
///
/// impl QueryFunction<Env> for __OpaqueSomeFunc {
///   <...>
///   fn invoke(&self, ...) {
///     self.0.some_func(<converted args>)
///   }
/// }
/// ```
///
/// and the generated QueryFunctions::get() will look something like:
///
/// ```ignore
///   fn get(&self, name: &str) -> Option<&dyn QueryFunction<Env>> {
///     match name {
///         "some_func" => Some(__OpaqueSomeFunc::ref_cast(self) as &dyn QueryFunction<Env>),
///         ...
///     }
///   }
/// ```
#[proc_macro_attribute]
pub fn query_module(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let result: syn::Result<_> = try {
        let parsed = parse::parse(attr.into(), input.into())?;
        codegen::codegen(parsed)?
    };

    match result {
        Ok(v) => v.into(),
        Err(e) => e.to_compile_error().into(),
    }
}
