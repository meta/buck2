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

//! Instructions serialized in byte array.

use std::collections::HashSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write;
use std::iter;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::slice;

use dupe::Dupe;
use either::Either;
use starlark_derive::StarlarkPagable;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::eval::bc::addr::BcAddr;
use crate::eval::bc::addr::BcAddrOffset;
use crate::eval::bc::addr::BcPtrAddr;
use crate::eval::bc::instr::BcInstr;
use crate::eval::bc::instr_impl::InstrEnd;
use crate::eval::bc::instr_impl::InstrIter;
use crate::eval::bc::opcode::BcOpcode;
use crate::eval::bc::opcode::BcOpcodeHandler;
use crate::eval::bc::repr::BC_INSTR_ALIGN;
use crate::eval::bc::repr::BcInstrHeader;
use crate::eval::bc::repr::BcInstrRepr;
use crate::eval::bc::slow_arg::BcInstrEndArg;
use crate::eval::bc::slow_arg::BcInstrSlowArg;
use crate::eval::bc::writer::BcStatementLocations;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::StringValue;
use crate::values::Value;

impl BcOpcode {
    /// Drop instruction at given address.
    unsafe fn drop_in_place(self, ptr: BcPtrAddr) {
        struct HandlerImpl<'b> {
            ptr: BcPtrAddr<'b>,
        }

        // Dropping an operand does not depend on the brand.
        impl BcOpcodeHandler<'static, ()> for HandlerImpl<'_> {
            fn handle<I: BcInstr<'static>>(self) {
                let HandlerImpl { ptr } = self;
                let instr = ptr.get_instr_mut::<'static, I>();
                unsafe {
                    ptr::drop_in_place(instr);
                }
            }
        }

        self.dispatch(HandlerImpl { ptr });
    }
}

/// Invoke drop for instructions in the buffer.
unsafe fn drop_instrs(instrs: &[u64]) {
    unsafe {
        let end = BcPtrAddr::for_slice_end(instrs);
        let mut ptr = BcPtrAddr::for_slice_start(instrs);
        while ptr != end {
            assert!(ptr < end);
            let opcode = ptr.get_opcode();
            opcode.drop_in_place(ptr);
            ptr = ptr.add(opcode.size_of_repr());
        }
    }
}

// Statically allocate a valid instruction buffer micro-optimization.
//
// Valid bytecode must end with `EndOfBc` instruction, otherwise evaluation overruns
// the instruction buffer.
//
// `BcInstrs` type need to have `Default` (it is convenient).
//
// Allocating a vec in `BcInstrs::default` is non-free.
//
// Assertion that `BcInstrs::instrs` is not empty is cheap but not free.
//
// But if `BcInstrs::instrs` is `Either` allocated instructions or a pointer to statically
// allocated instructions, then both `BcInstrs::default` is free
// and evaluation start [is free](https://rust.godbolt.org/z/3nEhWGo4Y).
fn empty_instrs() -> &'static [u64] {
    static END_OF_BC: std::sync::LazyLock<BcInstrRepr<'static, InstrEnd>> =
        std::sync::LazyLock::new(|| BcInstrRepr {
            header: BcInstrHeader::for_opcode(BcOpcode::End),
            arg: BcInstrEndArg {
                end_addr: BcAddr(0),
                slow_args: Vec::new(),
                dictnpop_spans: Box::default(),
                local_names: Box::default(),
            },
            _align: [],
        });
    unsafe {
        slice::from_raw_parts(
            &*END_OF_BC as *const BcInstrRepr<_> as *const u64,
            mem::size_of_val(&*END_OF_BC) / mem::size_of::<u64>(),
        )
    }
}

/// Marker for the "empty / default" bc variant of [`BcInstrs`].
///
/// Zero-sized; resolves to the shared static buffer from [`empty_instrs()`]
/// at read time. Using a ZST here (instead of storing `&'static [u64]`
/// directly in the field) avoids putting a `'static` pointer inside
/// `BcInstrs`, which would otherwise block serialization.
#[derive(Debug, Copy, Clone, Dupe, Default, PartialEq, Eq, StarlarkPagable)]
pub(crate) struct BcInstrsEmpty;

impl BcInstrsEmpty {
    /// Resolve to the shared static empty bc buffer ([`empty_instrs`]).
    ///
    /// A single `InstrEnd` terminator, zero-alloc, identical across all
    /// `BcInstrs::default()` instances in the process.
    #[inline]
    pub(crate) fn as_slice(self) -> &'static [u64] {
        empty_instrs()
    }
}

/// The instructions of a `Bc<'v>`: packed `BcInstrRepr<'v, I>` records, whose operands are
/// values at `'v`.
#[derive(ProvidesStaticType)]
pub(crate) struct BcInstrs<'v> {
    // `Left`: owned compiled buffer (`u64` for 8-byte alignment of
    // `BcInstrRepr`). `Right`: empty marker, resolved lazily.
    instrs: Either<Box<[u64]>, BcInstrsEmpty>,
    pub(crate) stmt_locs: BcStatementLocations<'v>,
    /// The brand of the operands packed in `instrs`.
    _marker: PhantomData<Value<'v>>,
}

// Manual `StarlarkSerialize` / `StarlarkDeserialize` for `BcInstrs`.
//
// The `Box<[u64]>` here is a packed buffer of `BcInstrRepr<I>` with embedded
// `Value`s — raw `u64` ser/de is unsound across processes. We walk via
// `self.iter()` and route each `I::Arg` through its own `StarlarkSerialize`.
//
// Wire format: u8 tag (0=empty, 1=compiled). Tag=1 is followed by a stream
// of `(u32 opcode, I::Arg)` terminated by `BcOpcode::End`. Then `stmt_locs`.

impl<'v> StarlarkSerialize for BcInstrs<'v> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        use pagable::PagableSerialize;
        match &self.instrs {
            Either::Right(empty) => {
                0u8.pagable_serialize(ctx.pagable())?;
                <BcInstrsEmpty as StarlarkSerialize>::starlark_serialize(empty, ctx)?;
            }
            Either::Left(_) => {
                1u8.pagable_serialize(ctx.pagable())?;
                for (ptr, _ip) in self.iter() {
                    let opcode = ptr.get_opcode();
                    (opcode as u32).pagable_serialize(ctx.pagable())?;
                    let mut handler = SerializeArgHandler::<'_, '_, 'v> {
                        ctx,
                        ptr,
                        result: Ok(()),
                        _marker: PhantomData,
                    };
                    opcode.dispatch(&mut handler);
                    handler.result?;
                }
            }
        }
        self.stmt_locs.starlark_serialize(ctx)?;
        Ok(())
    }
}

impl<'v> StarlarkDeserialize<'v> for BcInstrs<'v> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        use pagable::PagableDeserialize;
        let tag = u8::pagable_deserialize(ctx.pagable())?;
        let instrs = match tag {
            0 => Either::Right(<BcInstrsEmpty as StarlarkDeserialize>::starlark_deserialize(ctx)?),
            1 => {
                let mut writer = BcInstrsWriter::<'v>::new();
                loop {
                    let opcode_n = u32::pagable_deserialize(ctx.pagable())?;
                    let opcode = BcOpcode::by_number(opcode_n).ok_or_else(|| {
                        crate::Error::new_other(anyhow::anyhow!(
                            "invalid BcOpcode number on deserialize: {opcode_n}"
                        ))
                    })?;
                    let mut handler = DeserializeArgHandler {
                        ctx,
                        writer: &mut writer,
                        result: Ok(()),
                    };
                    opcode.dispatch(&mut handler);
                    handler.result?;
                    if opcode == BcOpcode::End {
                        break;
                    }
                }
                // Take the buffer without `finish()` (which appends another
                // `InstrEnd`); the deserialized one is already in place.
                let mut writer = writer;
                let buf = mem::take(&mut writer.instrs);
                mem::forget(writer);
                Either::Left(buf.into_boxed_slice())
            }
            _ => {
                return Err(crate::Error::new_other(anyhow::anyhow!(
                    "invalid BcInstrs tag: {tag}"
                )));
            }
        };
        let stmt_locs = BcStatementLocations::starlark_deserialize(ctx)?;
        Ok(BcInstrs {
            instrs,
            stmt_locs,
            _marker: PhantomData,
        })
    }
}

struct SerializeArgHandler<'a, 'b, 'v> {
    ctx: &'a mut dyn StarlarkSerializeContext,
    ptr: BcPtrAddr<'b>,
    result: crate::Result<()>,
    _marker: PhantomData<Value<'v>>,
}

impl<'a, 'b, 'v> BcOpcodeHandler<'v, ()> for &mut SerializeArgHandler<'a, 'b, 'v> {
    #[inline(always)]
    fn handle<I: BcInstr<'v>>(self) {
        // Dispatched on the opcode at `ptr`, so the record is `BcInstrRepr<I>`.
        let repr = self
            .ptr
            .get_instr_checked::<'v, I>()
            .expect("opcode/instr type mismatch");
        self.result = <I::Arg as StarlarkSerialize>::starlark_serialize(&repr.arg, self.ctx);
    }
}

struct DeserializeArgHandler<'a, 'de, 'w, 'v> {
    ctx: &'a mut dyn StarlarkDeserializeContext<'de, 'v>,
    writer: &'w mut BcInstrsWriter<'v>,
    result: crate::Result<()>,
}

impl<'a, 'de, 'w, 'v> BcOpcodeHandler<'v, ()> for &mut DeserializeArgHandler<'a, 'de, 'w, 'v> {
    #[inline(always)]
    fn handle<I: BcInstr<'v>>(self) {
        match <I::Arg as StarlarkDeserialize<'v>>::starlark_deserialize(self.ctx) {
            Ok(arg) => {
                self.writer.write::<I>(arg);
            }
            Err(e) => self.result = Err(e),
        }
    }
}

/// Raw instructions writer.
///
/// Higher level wrapper is `BcWriter`.
pub(crate) struct BcInstrsWriter<'v> {
    pub(crate) instrs: Vec<u64>,
    _marker: PhantomData<Value<'v>>,
}

impl<'v> Default for BcInstrs<'v> {
    fn default() -> Self {
        Self::for_instrs(Either::Right(BcInstrsEmpty), BcStatementLocations::new())
    }
}

impl<'v> Drop for BcInstrs<'v> {
    fn drop(&mut self) {
        match &self.instrs {
            Either::Left(heap_allocated) => unsafe {
                drop_instrs(heap_allocated);
            },
            Either::Right(BcInstrsEmpty) => {}
        }
    }
}

impl<'v> Drop for BcInstrsWriter<'v> {
    fn drop(&mut self) {
        unsafe {
            drop_instrs(&self.instrs);
        }
    }
}

pub(crate) struct PatchAddr {
    pub(crate) instr_start: BcAddr,
    pub(crate) arg: BcAddr,
}

impl<'v> BcInstrs<'v> {
    /// Borrow the raw `u64` buffer, resolving the `Empty` marker to
    /// [`empty_instrs()`] transparently.
    #[inline]
    fn as_slice(&self) -> &[u64] {
        match &self.instrs {
            Either::Left(boxed) => boxed,
            Either::Right(empty) => empty.as_slice(),
        }
    }

    pub(crate) fn start_ptr(&self) -> BcPtrAddr<'_> {
        BcPtrAddr::for_slice_start(self.as_slice())
    }

    pub(crate) fn for_instrs(
        instrs: Either<Box<[u64]>, BcInstrsEmpty>,
        stmt_locs: BcStatementLocations<'v>,
    ) -> Self {
        Self {
            instrs,
            stmt_locs,
            _marker: PhantomData,
        }
    }

    pub(crate) fn end(&self) -> BcAddr {
        BcAddr(
            self.as_slice()
                .len()
                .checked_mul(mem::size_of::<u64>())
                .unwrap()
                .try_into()
                .unwrap(),
        )
    }

    pub(crate) fn end_ptr(&self) -> BcPtrAddr<'_> {
        self.start_ptr().offset(self.end())
    }

    #[cfg(test)]
    pub(crate) fn opcodes(&self) -> Vec<BcOpcode> {
        let mut opcodes = Vec::new();
        let slice = self.as_slice();
        let end = BcPtrAddr::for_slice_end(slice);
        let mut ptr = BcPtrAddr::for_slice_start(slice);
        while ptr != end {
            assert!(ptr < end);
            let opcode = ptr.get_opcode();
            opcodes.push(opcode);
            ptr = ptr.add(opcode.size_of_repr());
        }
        opcodes
    }

    fn iter(&self) -> impl Iterator<Item = (BcPtrAddr<'_>, BcAddr)> {
        let mut next_ptr = self.start_ptr();
        iter::from_fn(move || {
            assert!(next_ptr <= self.end_ptr());
            if next_ptr == self.end_ptr() {
                None
            } else {
                let ptr = next_ptr;
                let ip = ptr.offset_from(self.start_ptr());
                next_ptr = next_ptr.add(ptr.get_opcode().size_of_repr());
                Some((ptr, ip))
            }
        })
    }

    fn end_arg(&self) -> Option<&BcInstrEndArg<'v>> {
        self.iter()
            .find_map(|(ptr, _ip)| ptr.get_instr_checked::<'v, InstrEnd>().map(|i| &i.arg))
    }

    pub(crate) fn fmt_impl(&self, f: &mut dyn Write, newline: bool) -> fmt::Result {
        let end_arg = self.end_arg();
        let ip_pad = if newline {
            let max_ip = self.iter().map(|(_, ip)| ip).max().unwrap_or(BcAddr(0));
            format!("{}", (max_ip).0).len()
        } else {
            0
        };

        let mut loop_ends = Vec::new();
        let mut jump_targets = HashSet::new();
        for (ptr, ip) in self.iter() {
            ptr.get_opcode().visit_jump_addr(ptr, ip, &mut |jump_addr| {
                jump_targets.insert(jump_addr);
            });
        }
        for (ptr, ip) in self.iter() {
            if loop_ends.last() == Some(&ip) {
                loop_ends.pop().unwrap();
            }
            let loop_pad = loop_ends.len() * 2;

            if newline {
                if let Some((loc, _continued)) = self.stmt_locs.stmt_at(ip) {
                    writeln!(f, "{:loop_pad$} {:ip_pad$}  # {}", "", "", loc.span.span)?;
                }
            } else {
                if ptr != self.start_ptr() {
                    write!(f, "; ")?;
                }
            }

            let opcode = ptr.get_opcode();
            if !jump_targets.is_empty() {
                if jump_targets.contains(&ip) {
                    write!(f, ">")?;
                } else if newline {
                    write!(f, " ")?;
                }
            }
            if newline {
                write!(f, "{:loop_pad$}", "")?;
            }
            write!(f, "{:ip_pad$}: {:?}", ip.0, opcode)?;
            if opcode != BcOpcode::End {
                // `End` args are too verbose and not really instruction args.
                opcode.fmt_append_arg(ptr, ip, end_arg, f)?;
            }
            if newline {
                writeln!(f)?;
            }
            if opcode == BcOpcode::Iter {
                let for_loop = ptr.get_instr::<'v, InstrIter>();
                loop_ends.push(ip.offset(for_loop.arg.4));
            }
        }
        Ok(())
    }

    pub(crate) fn dump_debug(&self) -> String {
        let mut w = String::new();
        self.fmt_impl(&mut w, true).unwrap();
        w
    }
}

impl<'v> Display for BcInstrs<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.fmt_impl(f, false)
    }
}

impl<'v> BcInstrsWriter<'v> {
    pub(crate) fn new() -> BcInstrsWriter<'v> {
        BcInstrsWriter {
            instrs: Vec::new(),
            _marker: PhantomData,
        }
    }

    fn instrs_len_bytes(&self) -> usize {
        self.instrs
            .len()
            .checked_mul(mem::size_of::<u64>())
            .unwrap()
    }

    pub(crate) fn ip(&self) -> BcAddr {
        BcAddr(self.instrs_len_bytes().try_into().unwrap())
    }

    pub(crate) fn write<I: BcInstr<'v>>(&mut self, arg: I::Arg) -> (BcAddr, *const I::Arg) {
        let repr = BcInstrRepr::<I>::new(arg);
        assert!(mem::size_of_val(&repr).is_multiple_of(mem::size_of::<u64>()));

        let ip = self.ip();

        let offset_bytes = self.instrs_len_bytes();
        self.instrs.resize(
            self.instrs.len() + mem::size_of_val(&repr) / mem::size_of::<u64>(),
            0,
        );
        unsafe {
            let ptr =
                (self.instrs.as_mut_ptr() as *mut u8).add(offset_bytes) as *mut BcInstrRepr<I>;
            ptr::write(ptr, repr);
            (ip, &(*ptr).arg)
        }
    }

    pub(crate) fn addr_to_patch(
        &self,
        instr_start: BcAddr,
        addr: *const BcAddrOffset,
    ) -> PatchAddr {
        unsafe { assert_eq!(*addr, BcAddrOffset::FORWARD) };
        let offset_bytes =
            unsafe { (addr as *const u8).offset_from(self.instrs.as_ptr() as *const u8) };
        assert!((offset_bytes as usize) < self.instrs_len_bytes());
        PatchAddr {
            instr_start,
            arg: BcAddr(offset_bytes as u32),
        }
    }

    pub(crate) fn patch_addr(&mut self, addr: PatchAddr) {
        unsafe {
            let mem_addr =
                (self.instrs.as_mut_ptr() as *mut u8).add(addr.arg.0 as usize) as *mut BcAddrOffset;
            assert!(*mem_addr == BcAddrOffset::FORWARD);
            *mem_addr = self.ip().offset_from(addr.instr_start);
            debug_assert!(((*mem_addr).0 as usize).is_multiple_of(BC_INSTR_ALIGN));
        }
    }

    pub(crate) fn finish(
        mut self,
        mut slow_args: Vec<(BcAddr, BcInstrSlowArg<'v>)>,
        dictnpop_spans: Box<[FrameSpan<'v>]>,
        mut stmt_locs: BcStatementLocations<'v>,
        local_names: Box<[StringValue<'v>]>,
    ) -> BcInstrs<'v> {
        slow_args.shrink_to_fit();
        stmt_locs.locs.shrink_to_fit();
        stmt_locs.stmts.shrink_to_fit();
        self.write::<InstrEnd>(BcInstrEndArg {
            end_addr: self.ip(),
            slow_args,
            dictnpop_spans,
            local_names,
        });
        // We cannot destructure `self` to fetch `instrs` because `Self` has `drop,
        // so we `mem::take`.
        let instrs = mem::take(&mut self.instrs);
        let instrs = instrs.into_boxed_slice();
        assert!((instrs.as_ptr() as usize).is_multiple_of(BC_INSTR_ALIGN));
        BcInstrs::for_instrs(Either::Left(instrs), stmt_locs)
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use crate::const_frozen_string;
    use crate::eval::bc::instr_impl::InstrConst;
    use crate::eval::bc::instr_impl::InstrPossibleGc;
    use crate::eval::bc::instr_impl::InstrReturn;
    use crate::eval::bc::instrs::BcInstrs;
    use crate::eval::bc::instrs::BcInstrsWriter;
    use crate::eval::bc::stack_ptr::BcSlot;
    use crate::eval::bc::writer::BcStatementLocations;
    use crate::values::Value;

    #[test]
    fn write() {
        let mut bc = BcInstrsWriter::<'static>::new();
        bc.write::<InstrPossibleGc>(());
        assert_eq!(1, bc.instrs.len());
        bc.write::<InstrPossibleGc>(());
        assert_eq!(2, bc.instrs.len());
    }

    /// Test `BcInstrs::default()` produces something valid.
    #[test]
    fn default() {
        assert_eq!("0: End", BcInstrs::<'static>::default().to_string());
    }

    #[test]
    fn display() {
        let local_names = Box::new([const_frozen_string!("abc").at()]);
        let mut bc = BcInstrsWriter::<'static>::new();
        bc.write::<InstrConst>((Value::new_bool(true), BcSlot(0).to_out()));
        bc.write::<InstrReturn>(BcSlot(0).to_in());
        let bc = bc.finish(
            Vec::new(),
            Box::default(),
            BcStatementLocations::new(),
            local_names,
        );
        if mem::size_of::<usize>() == 8 {
            assert_eq!(
                "0: Const True ->&abc; 24: Return &abc; 32: End",
                bc.to_string()
            );
            assert_eq!(
                " 0: Const True ->&abc\n24: Return &abc\n32: End\n",
                bc.dump_debug()
            );
        } else if mem::size_of::<usize>() == 4 {
            // Starlark doesn't work now on 32-bit CPU
        } else {
            panic!("unknown word size: {}", mem::size_of::<usize>());
        }
    }
}
