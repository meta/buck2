/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::Context as _;
use ruff_python_ast::Expr;
use serde::Deserialize;
use serde::Deserializer;
use serde::de::Error as _;
use vec1::Vec1;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(untagged)]
pub(crate) enum SortKey {
    Named(NamedSortKey),
    FirstOf(FirstOf),
    TupleItem(TupleItem),
    CallKeyword(CallKeyword),
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NamedSortKey {
    String,
    CallName,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FirstOf {
    pub(crate) first_of: Vec1<SortKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TupleItem {
    pub(crate) tuple_item: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallKeyword {
    pub(crate) call_keyword: String,
}

/// Configured structural sort keys, indexed by argument name.
///
/// Selectors are either a bare argument (`"items"`, the fallback for every
/// callee) or a callee-qualified `"callee.arg"` pair split on the *last* dot,
/// so `"module.rule.items"` means callee `"module.rule"` plus arg `"items"`
/// (callees may themselves contain dots). Empty selectors, empty segments,
/// and whitespace are rejected at deserialization so a key that could never
/// match fails loudly instead of silently never applying.
#[derive(Debug, Default)]
pub(crate) struct ListSortKeys {
    by_arg: HashMap<String, ArgSortKeys>,
}

/// Sort keys for one argument: an optional fallback plus callee-specific keys.
#[derive(Debug, Default)]
pub(crate) struct ArgSortKeys {
    fallback: Option<SortKey>,
    by_callee: HashMap<String, SortKey>,
}

impl<'de> Deserialize<'de> for ListSortKeys {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = HashMap::<String, SortKey>::deserialize(deserializer)?;
        let mut by_arg = HashMap::<String, ArgSortKeys>::new();

        for (selector, key) in entries {
            if selector.is_empty() {
                return Err(D::Error::custom("list sort key selector must not be empty"));
            }
            if selector.chars().any(|c| c.is_whitespace()) {
                return Err(D::Error::custom(format!(
                    "invalid list sort key selector `{selector}`: must not contain whitespace"
                )));
            }

            if let Some((callee, arg)) = selector.rsplit_once('.') {
                if callee.is_empty()
                    || arg.is_empty()
                    || callee.split('.').any(|segment| segment.is_empty())
                {
                    return Err(D::Error::custom(format!(
                        "invalid list sort key selector `{selector}`"
                    )));
                }
                by_arg
                    .entry(arg.to_owned())
                    .or_default()
                    .by_callee
                    .insert(callee.to_owned(), key);
            } else {
                by_arg.entry(selector).or_default().fallback = Some(key);
            }
        }

        Ok(Self { by_arg })
    }
}

impl ListSortKeys {
    pub(crate) fn is_empty(&self) -> bool {
        self.by_arg.is_empty()
    }

    pub(crate) fn for_arg(&self, arg: &str) -> Option<&ArgSortKeys> {
        self.by_arg.get(arg)
    }
}

impl ArgSortKeys {
    pub(crate) fn has_callee_keys(&self) -> bool {
        !self.by_callee.is_empty()
    }

    /// Resolve the key for `callee`, preferring the callee-specific entry
    /// over the argument-level fallback. Returns `None` when neither exists.
    ///
    /// A `None` callee always falls through to the fallback (callers must
    /// still consult the fallback — it is not implied by `has_callee_keys`).
    pub(crate) fn resolve(&self, callee: Option<&str>) -> Option<&SortKey> {
        callee
            .and_then(|callee| self.by_callee.get(callee))
            .or(self.fallback.as_ref())
    }
}

impl SortKey {
    pub(crate) fn from_json(json: &str) -> anyhow::Result<Self> {
        serde_json::from_str(json).context("invalid sort key JSON")
    }

    /// Extract the sort-key value for `expr`.
    ///
    /// Returns `Ok(None)` when the key does not apply to this expression
    /// shape (non-string, non-call, non-tuple) or the requested data is
    /// absent (e.g. a `call_keyword` key whose keyword is not present), and
    /// `Err` when the key applies but the value is unusable (non-string
    /// literal, out-of-range index, unsupported callee). Inside `first_of`, an `Err` aborts the
    /// fallback chain instead of continuing: inline directives rely on this
    /// fail-closed behavior, while configured keys get fail-open handling
    /// (skip the list) at the caller.
    pub(crate) fn extract<'a>(&self, expr: &'a Expr) -> anyhow::Result<Option<Cow<'a, str>>> {
        match self {
            Self::Named(NamedSortKey::String) => Ok(as_string_literal(expr).map(Cow::Borrowed)),
            Self::Named(NamedSortKey::CallName) => match expr {
                Expr::Call(call) => call_name(&call.func)
                    .map(|name| Some(Cow::Owned(name)))
                    .context("call has an unsupported callee"),
                _ => Ok(None),
            },
            Self::FirstOf(first_of) => {
                for key in &first_of.first_of {
                    if let Some(value) = key.extract(expr)? {
                        return Ok(Some(value));
                    }
                }
                Ok(None)
            }
            Self::TupleItem(tuple_item) => match expr {
                Expr::Tuple(tuple) => {
                    let item = tuple.elts.get(tuple_item.tuple_item).with_context(|| {
                        format!("tuple has no item at index {}", tuple_item.tuple_item)
                    })?;
                    match as_string_literal(item) {
                        Some(value) => Ok(Some(Cow::Borrowed(value))),
                        None => anyhow::bail!(
                            "tuple item at index {} is not a string literal",
                            tuple_item.tuple_item
                        ),
                    }
                }
                _ => Ok(None),
            },
            Self::CallKeyword(call_keyword) => match expr {
                Expr::Call(call) => {
                    let Some(keyword) = call.arguments.find_keyword(&call_keyword.call_keyword)
                    else {
                        return Ok(None);
                    };
                    match as_string_literal(&keyword.value) {
                        Some(value) => Ok(Some(Cow::Borrowed(value))),
                        None => anyhow::bail!(
                            "call keyword `{}` is not a string literal",
                            call_keyword.call_keyword
                        ),
                    }
                }
                _ => Ok(None),
            },
        }
    }
}

/// Borrow the string value of `expr`, or `None` when it is not a string literal.
///
/// Callers map `None` to their own outcome, which differs per key kind:
/// inapplicable shapes yield `Ok(None)` while present-but-invalid values
/// bail — see [`SortKey::extract`].
fn as_string_literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(string) => Some(string.value.to_str()),
        _ => None,
    }
}

/// Dotted name of a call callee (`f`, `module.rule`), or `None` for computed
/// callees (`factory()()`, `table["key"]()`) which no key can name.
///
/// Built with a single `String` allocation by pushing segments directly;
/// a dotted name is never contiguous in the AST, so zero-alloc would need
/// a structured key instead.
pub(crate) fn call_name(expr: &Expr) -> Option<String> {
    let mut name = String::new();
    push_qualified_name(expr, &mut name).then_some(name)
}

fn push_qualified_name(expr: &Expr, out: &mut String) -> bool {
    match expr {
        Expr::Name(name) => {
            out.push_str(name.id.as_str());
            true
        }
        Expr::Attribute(attr) => {
            if !push_qualified_name(&attr.value, out) {
                return false;
            }
            out.push('.');
            out.push_str(attr.attr.as_str());
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use ruff_python_ast::Stmt;

    use super::*;
    use crate::autofixes::parsed_module::ParsedModule;

    fn parse(json: &str) -> Result<SortKey, serde_json::Error> {
        serde_json::from_str(json)
    }

    fn extract(json: &str, expression: &str) -> anyhow::Result<Option<String>> {
        let source = format!("value = {expression}\n");
        let module = ParsedModule::parse(Cow::Owned(source))?;
        let Stmt::Assign(assign) = &module.stmts()[0] else {
            panic!("test input should parse as an assignment");
        };
        parse(json)
            .expect("sort key should parse")
            .extract(&assign.value)
            .map(|value| value.map(Cow::into_owned))
    }

    #[test]
    fn test_named_sort_keys() {
        assert_eq!(
            parse(r#""string""#).unwrap(),
            SortKey::Named(NamedSortKey::String)
        );
        assert_eq!(
            parse(r#""call_name""#).unwrap(),
            SortKey::Named(NamedSortKey::CallName)
        );
    }

    #[test]
    fn test_structural_sort_keys() {
        assert_eq!(
            parse(r#"{"tuple_item": 2}"#).unwrap(),
            SortKey::TupleItem(TupleItem { tuple_item: 2 })
        );
        assert_eq!(
            parse(r#"{"call_keyword": "name"}"#).unwrap(),
            SortKey::CallKeyword(CallKeyword {
                call_keyword: "name".to_owned(),
            })
        );
    }

    #[test]
    fn test_nested_first_of() {
        assert_eq!(
            parse(r#"{"first_of": ["string", {"tuple_item": 0}]}"#).unwrap(),
            SortKey::FirstOf(FirstOf {
                first_of: vec1::vec1![
                    SortKey::Named(NamedSortKey::String),
                    SortKey::TupleItem(TupleItem { tuple_item: 0 }),
                ],
            })
        );
    }

    #[test]
    fn test_rejects_invalid_sort_keys_during_deserialization() {
        for json in [
            r#"{"first_of": []}"#,
            r#"{"unknown": true}"#,
            r#"{"tuple_item": 0, "extra": true}"#,
            r#"{}"#,
        ] {
            assert!(parse(json).is_err(), "unexpectedly accepted {json}");
        }
    }

    #[test]
    fn test_list_sort_keys_resolve_callee_before_fallback() {
        let keys: ListSortKeys = serde_json::from_str(
            r#"{
                "items": "string",
                "module.rule.items": "call_name"
            }"#,
        )
        .expect("valid selector map");
        let item_keys = keys.for_arg("items").expect("items selector");

        assert!(!keys.is_empty());
        assert!(item_keys.has_callee_keys());
        assert_eq!(
            item_keys.resolve(Some("module.rule")),
            Some(&SortKey::Named(NamedSortKey::CallName))
        );
        assert_eq!(
            item_keys.resolve(Some("other.rule")),
            Some(&SortKey::Named(NamedSortKey::String))
        );
    }

    #[test]
    fn test_list_sort_keys_reject_invalid_selectors() {
        for json in [
            r#"{"": "string"}"#,
            r#"{".items": "string"}"#,
            r#"{"module.": "string"}"#,
            r#"{"module..items": "string"}"#,
            r#"{"module.rule. items": "string"}"#,
            r#"{" items": "string"}"#,
        ] {
            assert!(
                serde_json::from_str::<ListSortKeys>(json).is_err(),
                "unexpectedly accepted {json}"
            );
        }
    }

    #[test]
    fn test_list_sort_keys_resolve_none_without_fallback() {
        let keys: ListSortKeys = serde_json::from_str(r#"{"module.rule.items": "call_name"}"#)
            .expect("valid selector map");
        let item_keys = keys.for_arg("items").expect("items selector");

        assert_eq!(item_keys.resolve(None), None);
        assert_eq!(item_keys.resolve(Some("other.rule")), None);
    }

    #[test]
    fn test_extracts_structural_sort_keys() {
        assert_eq!(
            extract(r#""string""#, r#""value""#).unwrap(),
            Some("value".to_owned())
        );
        assert_eq!(
            extract(r#"{"tuple_item": 1}"#, r#"("ignored", "tuple")"#).unwrap(),
            Some("tuple".to_owned())
        );
        assert_eq!(
            extract(
                r#"{"call_keyword": "name"}"#,
                r#"factory(name = "keyword")"#
            )
            .unwrap(),
            Some("keyword".to_owned())
        );
        assert_eq!(
            extract(r#""call_name""#, "module.factory()").unwrap(),
            Some("module.factory".to_owned())
        );
    }

    #[test]
    fn test_call_name_joins_dotted_segments() {
        fn name_of(callee: &str) -> Option<String> {
            let source = format!("value = {callee}()\n");
            let module = ParsedModule::parse(Cow::Owned(source)).expect("parse");
            let Stmt::Assign(assign) = &module.stmts()[0] else {
                panic!("test input should parse as an assignment");
            };
            let ruff_python_ast::Expr::Call(call) = assign.value.as_ref() else {
                panic!("test input should parse as a call");
            };
            super::call_name(&call.func)
        }

        assert_eq!(name_of("factory"), Some("factory".to_owned()));
        assert_eq!(name_of("module.factory"), Some("module.factory".to_owned()));
        assert_eq!(name_of("a.b.c"), Some("a.b.c".to_owned()));
        assert_eq!(name_of("factory()"), None);
        assert_eq!(name_of("table[\"key\"]"), None);
    }

    #[test]
    fn test_first_of_uses_the_first_applicable_key() {
        assert_eq!(
            extract(
                r#"{"first_of": ["string", {"call_keyword": "name"}, "call_name"]}"#,
                r#"factory(name = "explicit")"#,
            )
            .unwrap(),
            Some("explicit".to_owned())
        );
    }

    #[test]
    fn test_first_of_propagates_candidate_errors() {
        // Intentionally strict: an invalid structural value in an earlier
        // candidate (e.g. non-string call keyword) surfaces as an error
        // instead of silently falling back. Inline directives rely on this
        // fail-closed behavior; configured keys get fail-open handling at
        // the caller, which skips the list on Err.
        assert!(
            extract(
                r#"{"first_of": [{"call_keyword": "name"}, "call_name"]}"#,
                r#"factory(name = dynamic)"#,
            )
            .is_err()
        );
    }

    #[test]
    fn test_extract_reports_invalid_structural_values() {
        assert!(extract(r#"{"tuple_item": 1}"#, r#"("only",)"#).is_err());
        assert!(extract(r#"{"call_keyword": "name"}"#, "factory(name = dynamic)").is_err());
    }

    #[test]
    fn test_extract_none_for_inapplicable_shapes() {
        // Inapplicable shapes yield `Ok(None)`, not an error: fail-closed
        // `first_of` chains rely on the distinction to keep trying later
        // candidates instead of aborting.
        assert_eq!(extract(r#""string""#, "factory()").unwrap(), None);
        assert_eq!(
            extract(r#"{"call_keyword": "name"}"#, "factory(other = 1)").unwrap(),
            None
        );
    }

    #[test]
    fn test_extract_rejects_present_non_string_tuple_item() {
        // The value is present but not a string literal: bail (`Err`) rather
        // than skip, exercising the `as_string_literal` none-arm shared with
        // the other key kinds.
        assert!(extract(r#"{"tuple_item": 1}"#, r#"("a", dynamic)"#).is_err());
    }
}
