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

use annotate_snippets::Annotation;
use annotate_snippets::AnnotationKind;
use annotate_snippets::Group;
use annotate_snippets::Level;
use annotate_snippets::Renderer;
use annotate_snippets::Snippet;

use crate::codemap::FileSpanRef;

/// Gets annotated snippets.
pub fn span_display(span: Option<FileSpanRef>, annotation_label: &str, color: bool) -> String {
    fn convert_span_to_snippet<'a>(span: FileSpanRef<'a>) -> Snippet<'a, Annotation<'a>> {
        let region = span.resolve_span();

        // we want the source_span to capture any whitespace ahead of the diagnostic span to
        // get the column numbers correct in the DisplayList, and any trailing source code
        // on the last line for context.
        let first_line_span = span.file.line_span(region.begin.line);
        let last_line_span = span.file.line_span(region.end.line);
        let source_span = span.span.merge(first_line_span).merge(last_line_span);
        let source = span.file.source_span(source_span);

        // We want to highlight the span, which needs to be relative to source, and in
        // characters.
        // Our spans are in terms of bytes, but our resolved spans in terms of characters.
        let range_start_byte = span
            .file
            .source_span(first_line_span)
            .chars()
            .take(region.begin.column)
            .map(char::len_utf8)
            .sum::<usize>();
        let range_len = span.source_span().len();

        Snippet::source(source)
            .line_start(1 + region.begin.line)
            .path(span.file.filename())
            .fold(false)
            .annotation(
                AnnotationKind::Primary.span(range_start_byte..range_start_byte + range_len),
            )
    }

    let title = Level::ERROR.primary_title(annotation_label);
    let message = Group::with_title(title).elements(span.map(convert_span_to_snippet));

    let renderer = if color {
        Renderer::styled()
    } else {
        Renderer::plain()
    };

    renderer.render(&[message]).to_string()
}
