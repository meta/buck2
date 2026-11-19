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

//! Call-related bytecode interpreter code.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;

use starlark_derive::StarlarkPagable;
use starlark_syntax::slice_vec_ext::VecExt;

use crate as starlark;
use crate::collections::symbol::symbol::Symbol;
use crate::eval::bc::frame::BcFramePtr;
use crate::eval::bc::instr_arg::BcInstrArg;
use crate::eval::bc::stack_ptr::BcSlotIn;
use crate::eval::bc::stack_ptr::BcSlotInRange;
use crate::eval::compiler::def::Def;
use crate::eval::runtime::arguments::ArgNames;
use crate::eval::runtime::arguments::ArgSymbol;
use crate::eval::runtime::arguments::ArgumentsFull;
use crate::eval::runtime::arguments::ArgumentsImpl;
use crate::eval::runtime::arguments::ArgumentsPos;
use crate::eval::runtime::arguments::ResolvedArgName;
use crate::values::StringValue;

/// A shape of call arguments: how a call instruction stores them and pops them from the stack.
///
/// Implemented by `'static` markers ([`FullArgs`], [`PosArgs`]) rather than by the argument
/// types, because the argument types carry the bytecode's brand and instruction types are
/// `'static`, see [`BcInstr`](crate::eval::bc::instr::BcInstr).
pub(crate) trait BcCallArgs<S: ArgSymbol>: 'static {
    /// The operand.
    type Arg<'v>: BcInstrArg<'v>;

    fn pop_from_stack<'a, 'v>(
        arg: &'a Self::Arg<'v>,
        frame: BcFramePtr<'v>,
    ) -> ArgumentsFull<'v, 'a, S>;
}

/// A shape of call arguments for `def` call, see [`BcCallArgs`].
pub(crate) trait BcCallArgsForDef: 'static {
    /// The operand.
    type Arg<'v>: BcInstrArg<'v>;
    type Args<'v, 'a>: ArgumentsImpl<'v, 'a, ArgSymbol = ResolvedArgName>
    where
        'v: 'a;

    fn pop_from_stack<'a, 'v>(arg: &'a Self::Arg<'v>, stack: BcFramePtr<'v>) -> Self::Args<'v, 'a>;
}

/// Full call arguments: positional, named, star and star-star. All taken from the stack.
#[derive(Debug, StarlarkPagable)]
// `ArgSymbol` is pagable at every brand; a per-brand bound on top of it would be ambiguous.
#[starlark_pagable(bound = "")]
pub(crate) struct BcCallArgsFull<'v, S: ArgSymbol> {
    pub(crate) pos_named: BcSlotInRange,
    pub(crate) names: Box<[(S, StringValue<'v>)]>,
    pub(crate) args: Option<BcSlotIn>,
    pub(crate) kwargs: Option<BcSlotIn>,
}

/// Positional-only call arguments, from stack.
#[derive(Debug, StarlarkPagable)]
pub(crate) struct BcCallArgsPos {
    /// Range of positional arguments.
    pub(crate) pos: BcSlotInRange,
}

/// The [`BcCallArgs`] of [`BcCallArgsFull`].
pub(crate) struct FullArgs<S>(PhantomData<S>);

/// The [`BcCallArgs`] of [`BcCallArgsPos`].
pub(crate) struct PosArgs;

impl<'v, S: ArgSymbol> BcCallArgsFull<'v, S> {
    /// Number of positional arguments.
    fn pos(&self) -> u32 {
        assert!(self.pos_named.len() >= (self.names.len() as u32));
        self.pos_named.len() - (self.names.len() as u32)
    }
}

impl<'v> BcCallArgsFull<'v, Symbol> {
    pub(crate) fn resolve(self, def: &Def<'v>) -> BcCallArgsFull<'v, ResolvedArgName> {
        let BcCallArgsFull {
            pos_named,
            names,
            args,
            kwargs,
        } = self;
        BcCallArgsFull {
            pos_named,
            names: names
                .into_vec()
                .into_map(|(name, value)| (def.resolve_arg_name(name.as_str_hashed()), value))
                .into_boxed_slice(),
            args,
            kwargs,
        }
    }
}

impl<'v, S: ArgSymbol> Display for BcCallArgsFull<'v, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let BcCallArgsFull {
            pos_named,
            names,
            args,
            kwargs,
        } = self;
        write!(f, "{pos_named}")?;
        // Number of positional arguments.
        if self.pos() != 0 {
            write!(f, " {}", self.pos())?;
        }
        // Named arguments.
        for (_, name) in &**names {
            write!(f, " {}", name.as_str())?;
        }
        // Star argument?
        if let Some(args) = args {
            write!(f, " *{args}")?;
        }
        // Star-star argument?
        if let Some(kwargs) = kwargs {
            write!(f, " **{kwargs}")?;
        }
        Ok(())
    }
}

impl<S: ArgSymbol> BcCallArgs<S> for FullArgs<S> {
    type Arg<'v> = BcCallArgsFull<'v, S>;

    #[inline]
    fn pop_from_stack<'a, 'v>(
        arg: &'a BcCallArgsFull<'v, S>,
        stack: BcFramePtr<'v>,
    ) -> ArgumentsFull<'v, 'a, S> {
        let pos_named = stack.get_bc_slot_range(arg.pos_named);
        let (pos, named) = pos_named.split_at(pos_named.len() - arg.names.len());
        let args = arg.args.map(|slot| stack.get_bc_slot(slot));
        let kwargs = arg.kwargs.map(|slot| stack.get_bc_slot(slot));
        ArgumentsFull {
            pos,
            named,
            names: ArgNames::new_unique(&arg.names),
            args,
            kwargs,
        }
    }
}

impl<S: ArgSymbol> BcCallArgs<S> for PosArgs {
    type Arg<'v> = BcCallArgsPos;

    #[inline]
    fn pop_from_stack<'a, 'v>(
        arg: &'a BcCallArgsPos,
        stack: BcFramePtr<'v>,
    ) -> ArgumentsFull<'v, 'a, S> {
        let pos = stack.get_bc_slot_range(arg.pos);
        ArgumentsFull {
            pos,
            named: &[],
            names: ArgNames::new_unique(&[]),
            args: None,
            kwargs: None,
        }
    }
}

impl BcCallArgsForDef for FullArgs<ResolvedArgName> {
    type Arg<'v> = BcCallArgsFull<'v, ResolvedArgName>;
    type Args<'v, 'a>
        = ArgumentsFull<'v, 'a, ResolvedArgName>
    where
        'v: 'a;

    #[inline]
    fn pop_from_stack<'a, 'v>(
        arg: &'a BcCallArgsFull<'v, ResolvedArgName>,
        stack: BcFramePtr<'v>,
    ) -> ArgumentsFull<'v, 'a, ResolvedArgName> {
        let pos_named = stack.get_bc_slot_range(arg.pos_named);
        let (pos, named) = pos_named.split_at(pos_named.len() - arg.names.len());
        let args = arg.args.map(|slot| stack.get_bc_slot(slot));
        let kwargs = arg.kwargs.map(|slot| stack.get_bc_slot(slot));
        ArgumentsFull {
            pos,
            named,
            names: ArgNames::new_unique(&arg.names),
            args,
            kwargs,
        }
    }
}

impl BcCallArgsForDef for PosArgs {
    type Arg<'v> = BcCallArgsPos;
    type Args<'v, 'a>
        = ArgumentsPos<'v, 'a, ResolvedArgName>
    where
        'v: 'a;

    #[inline]
    fn pop_from_stack<'a, 'v>(
        arg: &'a BcCallArgsPos,
        stack: BcFramePtr<'v>,
    ) -> ArgumentsPos<'v, 'a, ResolvedArgName> {
        let pos = stack.get_bc_slot_range(arg.pos);
        ArgumentsPos {
            pos,
            names: PhantomData,
        }
    }
}
