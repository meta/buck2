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

/// Initialize to `&'static FrameSpan<'static>` with rust file and line number; `FrameSpan::at`
/// brings it to the brand of the heap in use.
macro_rules! rust_loc {
    () => {{
        use std::sync::LazyLock;

        use crate::codemap::CodeMap;
        use crate::codemap::NativeCodeMap;
        use crate::eval::runtime::frame_span::FrameSpan;
        use crate::eval::runtime::heap_file_span::HeapFileSpan;

        static NATIVE_CODEMAP: NativeCodeMap = NativeCodeMap::new(file!(), line!(), column!());
        pagable::static_value!(
            NATIVE_CODEMAP_STATIC: NativeCodeMap = &NATIVE_CODEMAP,
            starlark_syntax::codemap::NativeCodeMapStaticEntry
        );
        // @no_impl: StaticValueRegistered for StarlarkAny<CodeMap> is already
        // implemented by the static_starlark_any! invocation in heap_file_span.rs.
        crate::static_starlark_any!(
            @no_impl CODEMAP: CodeMap = NativeCodeMap::to_codemap(NATIVE_CODEMAP_STATIC)
        );
        static FRAME_SPAN: LazyLock<FrameSpan<'static>> = LazyLock::new(|| {
            FrameSpan::new(HeapFileSpan::new_unchecked(
                CODEMAP.at(),
                NativeCodeMap::FULL_SPAN,
            ))
        });
        &*FRAME_SPAN
    }};
}

pub(crate) use rust_loc;

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;
    use starlark_syntax::error::StarlarkResultExt;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::eval::Arguments;
    use crate::eval::Evaluator;
    use crate::values::Value;

    #[starlark_module]
    fn rust_loc_globals(globals: &mut GlobalsBuilder) {
        fn invoke<'v>(f: Value<'v>, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
            f.invoke_with_loc(Some(rust_loc!().at()), &Arguments::default(), eval)
                .into_anyhow_result()
        }
    }

    #[test]
    fn test_rust_loc() {
        let mut a = Assert::new();
        a.globals_add(rust_loc_globals);
        let err = a.fail("invoke(fail)", "");
        let err = err.to_string();
        // Stack trace should contain invocation in `invoke`.
        assert!(
            // Make test compatible with Windows.
            err.replace('\\', "/")
                .contains("src/eval/runtime/rust_loc.rs"),
            "output: {err:?}"
        );
        assert!(err.contains("<native>"), "output: {err:?}");
    }
}
