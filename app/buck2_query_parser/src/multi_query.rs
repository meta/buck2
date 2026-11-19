/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::placeholder::QUERY_PERCENT_S_PLACEHOLDER;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum EvalQueryError {
    #[error(
        "Query args supplied without any `%s` placeholder in the query, got args {}. \
         Only the first argument is the query; each later argument is substituted for `%s` in it. \
         Use `\"owner('%s')\" a.rs b.rs` to run the query once per argument, \
         or `\"owner('a.rs') + owner('b.rs')\"` to combine expressions in a single query",
        .0.iter().map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", ")
    )]
    ArgsWithoutPlaceholder(Vec<String>),
    #[error("Placeholder `%s` in query argument `{0}`")]
    PlaceholderInPattern(String),
}

pub struct MultiQueryItem {
    pub arg: String,
    pub query: String,
}

/// Parsed query with optional `%s` placeholder and optional query args.
pub enum MaybeMultiQuery {
    SingleQuery(String),
    MultiQuery(Vec<MultiQueryItem>),
}

impl MaybeMultiQuery {
    pub fn parse(
        query: &str,
        args: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> buck2_error::Result<MaybeMultiQuery> {
        if query.contains(QUERY_PERCENT_S_PLACEHOLDER) {
            // We'd really like the query args to only be literals (file or target).
            // If that didn't work, we'd really like query args to be well-formed expressions.
            // Unfortunately Buck1 just substitutes in arbitrarily strings, where the query
            // or query_args may not form anything remotely valid.
            // We have to be backwards compatible :(
            let queries = args
                .into_iter()
                .map(|arg| {
                    let arg = arg.as_ref().to_owned();
                    if arg.contains(QUERY_PERCENT_S_PLACEHOLDER) {
                        return Err(EvalQueryError::PlaceholderInPattern(arg).into());
                    }
                    let query = query.replace(QUERY_PERCENT_S_PLACEHOLDER, &arg);
                    Ok(MultiQueryItem { arg, query })
                })
                .collect::<buck2_error::Result<_>>()?;
            Ok(MaybeMultiQuery::MultiQuery(queries))
        } else {
            let args: Vec<String> = args.into_iter().map(|q| q.as_ref().to_owned()).collect();
            if !args.is_empty() {
                Err(EvalQueryError::ArgsWithoutPlaceholder(args).into())
            } else {
                Ok(MaybeMultiQuery::SingleQuery(query.to_owned()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_without_placeholder_suggests_working_forms() {
        let err = match MaybeMultiQuery::parse("owner('a.rs')", ["owner('b.rs')"]) {
            Err(e) => format!("{e}"),
            Ok(_) => panic!("expected error"),
        };
        assert!(err.contains("got args `owner('b.rs')`"), "{err}");
        assert!(err.contains("\"owner('%s')\" a.rs b.rs"), "{err}");
        assert!(err.contains("\"owner('a.rs') + owner('b.rs')\""), "{err}");
    }
}
