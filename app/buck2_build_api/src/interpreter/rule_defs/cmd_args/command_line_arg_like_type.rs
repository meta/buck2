/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;

use starlark::typing::Ty;

pub struct CommandLineArgLikeImpl {
    pub ty: fn() -> Ty,
}

pub mod __macro_refs {
    pub use linkme;
}

#[macro_export]
macro_rules! command_line_arg_like_impl {
    ($ty:expr) => {
        {
            use $crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::__macro_refs::linkme;

            #[linkme::distributed_slice(
                $crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::COMMAND_LINE_ARG_LIKE_IMPLS
            )]
            #[linkme(crate = $crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::__macro_refs::linkme)]
            static IMPL: $crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::CommandLineArgLikeImpl =
                $crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::CommandLineArgLikeImpl { ty: || $ty };
        }
    };
}

pub use command_line_arg_like_impl;

#[linkme::distributed_slice]
pub static COMMAND_LINE_ARG_LIKE_IMPLS: [CommandLineArgLikeImpl] = [..];

pub(crate) fn command_line_arg_like_ty() -> &'static Ty {
    static TY: LazyLock<Ty> = LazyLock::new(|| {
        assert!(!COMMAND_LINE_ARG_LIKE_IMPLS.is_empty());
        let ty = Ty::unions(
            COMMAND_LINE_ARG_LIKE_IMPLS
                .iter()
                .map(|impl_| (impl_.ty)())
                .collect(),
        );
        assert_ne!(ty, Ty::any());
        assert_ne!(ty, Ty::never());
        ty
    });
    &TY
}

#[cfg(test)]
mod tests {
    use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_ty;

    #[test]
    fn test_command_arg_like_ty() {
        // Trigger assertions.
        command_line_arg_like_ty();
    }
}
