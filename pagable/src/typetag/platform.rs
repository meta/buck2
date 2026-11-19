/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Platform-specific emission of generic typetag registration constructors.
//!
//! Each monomorphization emits a pointer to its registration helper into the
//! platform's program-constructor section (`.init_array` on ELF,
//! `__mod_init_func` on Mach-O, `.CRT$XCU` on PE/COFF), so the loader or CRT
//! runs every helper when the image containing it is loaded.
//!
//! Constructors are used instead of a walkable named section because
//! `__start_`/`__stop_`-style encapsulation symbols bind per linked image: a
//! walker compiled into the pagable crate only sees the section of the image
//! that contains pagable, and silently misses registrations in every other
//! image (e.g. an executable whose dependencies are linked as shared
//! libraries). Constructor sections are run by the loader for each image, so
//! every image registers its own monomorphizations.

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __pagable_emit_generic_typetag_registration {
    ($register:path) => {
        // SAFETY: This assembly emits one pointer-sized `.init_array` record
        // that relocates to the monomorphized registration helper; the loader
        // calls the helper once when the containing image is loaded. No
        // instructions are emitted at the expansion site.
        unsafe {
            core::arch::asm!(
                concat!(
                    ".pushsection .init_array,\"aw\",@init_array\n",
                    ".p2align 3\n",
                    ".quad {register}\n",
                    ".popsection",
                ),
                register = sym $register,
                options(nostack, preserves_flags),
            );
        }
    };
}

#[cfg(all(target_os = "macos", target_pointer_width = "64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __pagable_emit_generic_typetag_registration {
    ($register:path) => {
        // SAFETY: This assembly emits one pointer-sized `__mod_init_func`
        // record that relocates to the monomorphized registration helper; the
        // loader calls the helper once when the containing image is loaded.
        // No instructions are emitted at the expansion site.
        unsafe {
            core::arch::asm!(
                concat!(
                    ".pushsection __DATA,__mod_init_func,mod_init_funcs\n",
                    ".p2align 3\n",
                    ".quad {register}\n",
                    ".popsection",
                ),
                register = sym $register,
                options(nostack, preserves_flags),
            );
        }
    };
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __pagable_emit_generic_typetag_registration {
    ($register:path) => {
        // SAFETY: This assembly emits one pointer-sized `.CRT$XCU` record
        // that relocates to the monomorphized registration helper; the CRT
        // calls the helper once during image initialization. No instructions
        // are emitted at the expansion site.
        unsafe {
            core::arch::asm!(
                concat!(
                    ".pushsection .CRT$XCU,\"dr\"\n",
                    ".p2align 3\n",
                    ".quad {register}\n",
                    ".popsection",
                ),
                register = sym $register,
                // Windows x86_64 normally uses Intel inline assembly, but
                // LLVM's Intel inline-asm printer consumes `$$` without
                // emitting `$`. The CRT initializer subsection needs the
                // literal `$XCU`, so use AT&T mode for this directive-only
                // assembly block.
                options(att_syntax, nostack, preserves_flags),
            );
        }
    };
}

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __pagable_emit_generic_typetag_registration {
    ($register:path) => {
        // SAFETY: This assembly emits one pointer-sized `.CRT$XCU` record
        // that relocates to the monomorphized registration helper; the CRT
        // calls the helper once during image initialization. No instructions
        // are emitted at the expansion site.
        unsafe {
            core::arch::asm!(
                concat!(
                    ".pushsection .CRT$XCU,\"dr\"\n",
                    ".p2align 3\n",
                    ".quad {register}\n",
                    ".popsection",
                ),
                register = sym $register,
                // Unlike the x86_64 arm, no `att_syntax` here: that option is
                // rejected outside x86, and the `$$`-consuming Intel
                // inline-asm printer it works around is x86-only, so `$XCU`
                // survives as written.
                options(nostack, preserves_flags),
            );
        }
    };
}

#[cfg(target_family = "wasm")]
#[doc(hidden)]
#[macro_export]
macro_rules! __pagable_emit_generic_typetag_registration {
    ($register:path) => {
        // Wasm offers no inline assembly to emit a constructor record and no
        // loader to run one, so generic instantiations cannot be registered
        // eagerly. They are not registered at all: deserializing a generic
        // typetag on wasm fails with "Unknown type tag". Reference the
        // helper so the monomorphization is not flagged as dead code.
        {
            let _ = $register;
        }
    };
}

#[cfg(not(any(
    all(
        any(target_os = "linux", target_os = "macos"),
        target_pointer_width = "64",
    ),
    all(
        target_os = "windows",
        any(target_arch = "x86_64", target_arch = "aarch64"),
    ),
    target_family = "wasm",
)))]
#[doc(hidden)]
#[macro_export]
macro_rules! __pagable_emit_generic_typetag_registration {
    ($register:path) => {
        compile_error!(
            "generic pagable typetag registration supports only 64-bit Linux/macOS, x86_64/aarch64 Windows, and wasm (unregistered)"
        );
    };
}
