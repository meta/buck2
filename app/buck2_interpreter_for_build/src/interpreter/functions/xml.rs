/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::BytesStart;
use quick_xml::events::Event;
use quick_xml::name::QName;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Heap;
use starlark::values::Value;
use starlark::values::dict::AllocDict;
use starlark::values::list::AllocList;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum XmlDecodeError {
    #[error("xml.decode: unexpected closing tag without matching open tag")]
    UnexpectedClosingTag,
    #[error("xml.decode: no root element found in XML input")]
    NoRootElement,
    #[error("xml.decode: {0}")]
    XmlError(String),
}

/// An XML element collected during parsing, before allocation on the Starlark heap.
struct XmlElement {
    tag: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<XmlElement>,
}

impl XmlElement {
    fn alloc<'v>(self, heap: Heap<'v>) -> Value<'v> {
        let children: Vec<Value<'v>> = self.children.into_iter().map(|c| c.alloc(heap)).collect();

        let tag = heap.alloc(&self.tag).to_value();
        let attrs = heap
            .alloc(AllocDict(self.attrs.into_iter().map(|(k, v)| {
                (heap.alloc(&k).to_value(), heap.alloc(&v).to_value())
            })))
            .to_value();
        let text = heap.alloc(&self.text).to_value();
        let children_val = heap.alloc(AllocList(children)).to_value();

        heap.alloc(AllocDict([
            ("tag", tag),
            ("attrs", attrs),
            ("text", text),
            ("children", children_val),
        ]))
        .to_value()
    }
}

fn parse_attrs(e: &BytesStart) -> Result<Vec<(String, String)>, XmlDecodeError> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(|e| XmlDecodeError::XmlError(e.to_string()))?;
            let key = a.key.as_ref().to_owned();
            let val = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| XmlDecodeError::XmlError(e.to_string()))?
                .into_owned();
            Ok((key, val))
        })
        .collect()
}

/// Parse an XML string into a tree of `XmlElement`s using quick-xml's streaming reader.
fn parse_xml(input: &str) -> Result<XmlElement, XmlDecodeError> {
    let mut buf = Vec::new();
    let mut reader = Reader::from_str(input);

    let mut stack: Vec<XmlElement> = Vec::new();
    let mut root: Option<XmlElement> = None;
    let mut next_event: Option<Event> = None;

    loop {
        let mut event = match next_event {
            Some(next_event) => next_event,
            None => {
                buf.clear();
                reader
                    .read_event_into(&mut buf)
                    .map_err(|e| XmlDecodeError::XmlError(e.to_string()))?
            }
        };
        next_event = None;
        match event {
            Event::Start(e) => {
                let QName(tag) = e.name();
                let attrs = parse_attrs(&e)?;
                stack.push(XmlElement {
                    tag: tag.to_owned(),
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Event::End(_) => {
                let elem = stack.pop().ok_or(XmlDecodeError::UnexpectedClosingTag)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(elem);
                } else {
                    root = Some(elem);
                }
            }
            Event::Empty(e) => {
                let QName(tag) = e.name();
                let attrs = parse_attrs(&e)?;
                let elem = XmlElement {
                    tag: tag.to_owned(),
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(elem);
                } else {
                    root = Some(elem);
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => {
                let mut text = String::new();
                loop {
                    // Consume consecutive Text and GeneralRef events.
                    match event {
                        Event::Text(e) => text.push_str(&e),
                        Event::GeneralRef(e) => {
                            if e.is_char_ref() {
                                if let Some(ch) = e
                                    .resolve_char_ref()
                                    .map_err(|e| XmlDecodeError::XmlError(e.to_string()))?
                                {
                                    text.push(ch);
                                }
                            } else if let Some(value) = quick_xml::escape::resolve_xml_entity(&e) {
                                text.push_str(value);
                            }
                        }
                        _ => {
                            next_event = Some(event);
                            break;
                        }
                    }
                    buf.clear();
                    event = reader
                        .read_event_into(&mut buf)
                        .map_err(|e| XmlDecodeError::XmlError(e.to_string()))?;
                }
                if let Some(current) = stack.last_mut() {
                    current.text.push_str(text.trim_ascii());
                }
            }
            Event::CData(e) => {
                let text = e.into_inner();
                if let Some(current) = stack.last_mut() {
                    current.text.push_str(&text);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    root.ok_or(XmlDecodeError::NoRootElement)
}

#[starlark_module]
fn xml_members(globals: &mut GlobalsBuilder) {
    /// Decodes an XML string into a Starlark dict tree.
    ///
    /// Each XML element becomes a dict with four keys:
    /// - `"tag"`: the element name (string)
    /// - `"attrs"`: a dict of attribute name→value pairs
    /// - `"text"`: the direct text content of the element (string)
    /// - `"children"`: a list of child element dicts
    ///
    /// ```python
    /// doc = xml.decode('<root attr="1"><child>hello</child></root>')
    /// doc["tag"]  # "root"
    /// doc["attrs"]["attr"]  # "1"
    /// doc["children"][0]["text"]  # "hello"
    /// ```
    fn decode<'v>(
        #[starlark(require = pos)] x: &str,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        let root = parse_xml(x).map_err(buck2_error::Error::from)?;
        Ok(root.alloc(heap))
    }
}

pub(crate) fn register_xml(globals: &mut GlobalsBuilder) {
    globals.namespace("xml", xml_members);
}

#[cfg(test)]
mod tests {
    use starlark::assert::Assert;

    use crate::interpreter::functions::xml::register_xml;

    fn assert_with_xml() -> Assert<'static> {
        let mut a = Assert::new();
        a.globals_add(|gb| {
            gb.namespace("__internal__", register_xml);
        });
        a
    }

    #[test]
    fn test_xml_decode_simple() {
        assert_with_xml().eq("__internal__.xml.decode('<root/>')[\"tag\"]", "'root'");
    }

    #[test]
    fn test_xml_decode_attrs() {
        assert_with_xml().eq(
            "__internal__.xml.decode('<r a=\"1\" b=\"2\"/>')[\"attrs\"][\"a\"]",
            "'1'",
        );
    }

    #[test]
    fn test_xml_decode_text() {
        assert_with_xml().eq(
            "__internal__.xml.decode('<msg>hello world</msg>')[\"text\"]",
            "'hello world'",
        );
    }

    #[test]
    fn test_xml_decode_children() {
        assert_with_xml().eq(
            "len(__internal__.xml.decode('<root><a/><b/><c/></root>')[\"children\"])",
            "3",
        );
    }

    #[test]
    fn test_xml_decode_nested() {
        assert_with_xml().eq(
            "__internal__.xml.decode('<root><child>text</child></root>')[\"children\"][0][\"text\"]",
            "'text'",
        );
    }

    #[test]
    fn test_xml_decode_junit() {
        assert_with_xml().is_true(
            r#"
doc = __internal__.xml.decode('<testsuite name="suite1" tests="2"><testcase name="test1"/><testcase name="test2"><failure message="oops"/></testcase></testsuite>')
doc["tag"] == "testsuite" and doc["attrs"]["name"] == "suite1" and len(doc["children"]) == 2
"#,
        );
    }
}
