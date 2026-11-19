/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementation of the cli and query_* attr query language.

use std::iter;

use allocative::Allocative;
use buck2_query_parser::spanned::Spanned;
use itertools::Either;
use variant_name::VariantName;

use crate::query::environment::QueryTarget;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::file_set::FileSet;
use crate::query::syntax::simple::eval::multi_query::MultiQueryResult;
use crate::query::syntax::simple::eval::set::TargetSet;

pub enum QueryEvaluationResult<T: QueryTarget> {
    Single(QueryEvaluationValue<T>),
    Multiple(MultiQueryResult<T>),
}

impl<T: QueryTarget> QueryEvaluationResult<T> {
    /// All the targets from all query results.
    pub fn targets(&self) -> impl Iterator<Item = buck2_error::Result<&T>> {
        match self {
            QueryEvaluationResult::Single(v) => Either::Left(v.targets()),
            QueryEvaluationResult::Multiple(v) => Either::Right(v.targets()),
        }
    }
}

/// Used as a value in query evaluation, may appear in arguments to functions, results of functions etc.
#[derive(Debug, VariantName, Eq, PartialEq)]
pub enum QueryValue<T: QueryTarget> {
    None,
    String(String),
    Integer(u64),
    TargetSet(TargetSet<T>),
    FileSet(FileSet),
}

/// Used as a value in query evaluation where sets are valid, may appear in arguments to functions, results of functions etc.
#[derive(Debug)]
pub enum QueryValueSet<T: QueryTarget> {
    TargetSet(TargetSet<T>),
    FileSet(FileSet),
}

/// Used as a value in query evaluation where the depth of the graph traversal is specified.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Default, Allocative)]
pub enum QueryValueDepth {
    #[default]
    Unbounded,
    Bounded(u32),
}

impl QueryValueDepth {
    #[inline]
    pub fn is_unbounded(&self) -> bool {
        match self {
            QueryValueDepth::Unbounded => true,
            QueryValueDepth::Bounded(_) => false,
        }
    }

    #[inline]
    pub fn bound(&self) -> Option<u32> {
        match self {
            QueryValueDepth::Unbounded => None,
            QueryValueDepth::Bounded(bound) => Some(*bound),
        }
    }
}

impl From<u32> for QueryValueDepth {
    fn from(v: u32) -> Self {
        // For unbounded traversals, buck1 recommends specifying a large value. We'll accept either a negative (like -1) or
        // a large value as unbounded. We can't just call it optional because args are positional only in the query syntax
        // and so to specify a filter you need to specify a depth.
        if (0..1_000_000_000).contains(&v) {
            QueryValueDepth::Bounded(v)
        } else {
            QueryValueDepth::Unbounded
        }
    }
}

impl From<Option<u32>> for QueryValueDepth {
    fn from(v: Option<u32>) -> Self {
        match v {
            None => QueryValueDepth::Unbounded,
            Some(v) => v.into(),
        }
    }
}

impl From<Option<i32>> for QueryValueDepth {
    fn from(v: Option<i32>) -> Self {
        match v {
            None => QueryValueDepth::Unbounded,
            Some(v) => {
                // Historically we treated all negative values as large u32 values in the `From<u32>` implementation.
                (v as u32).into()
            }
        }
    }
}

/// Used as the final result of evaluating a query. A literal at the top-level is treated specially and so this has
/// a more limited set of possibilities than a general QueryValue (for example `//foo/...` becomes a TargetSet in
/// `buck query //foo/...` rather than being a String).
#[derive(Debug, VariantName)]
pub enum QueryEvaluationValue<T: QueryTarget> {
    TargetSet(TargetSet<T>),
    FileSet(FileSet),
}

impl<T: QueryTarget> QueryEvaluationValue<T> {
    /// What the value holds, for error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            QueryEvaluationValue::TargetSet(_) => "a set of targets",
            QueryEvaluationValue::FileSet(_) => "a set of files",
        }
    }

    pub fn try_into_targets(self) -> buck2_error::Result<TargetSet<T>> {
        match self {
            QueryEvaluationValue::TargetSet(targets) => Ok(targets),
            v => Err(QueryError::InvalidType {
                expected: "targets",
                actual: v.variant_name(),
            }
            .into()),
        }
    }

    pub(crate) fn targets(&self) -> impl Iterator<Item = buck2_error::Result<&T>> {
        match self {
            QueryEvaluationValue::TargetSet(targets) => Either::Left(targets.iter().map(Ok)),
            v => Either::Right(iter::once(Err(QueryError::InvalidType {
                expected: "targets",
                actual: v.variant_name(),
            }
            .into()))),
        }
    }
}

pub type QueryResult<T> = Result<Spanned<T>, Spanned<QueryError>>;

pub trait QueryResultExt<T> {
    fn into_buck2_error(self, query: &str) -> buck2_error::Result<T>;
}

impl<T> QueryResultExt<T> for QueryResult<T> {
    fn into_buck2_error(self, query: &str) -> buck2_error::Result<T> {
        match self {
            Ok(v) => Ok(v.value),
            Err(err) => Err(QueryError::convert_error(err, query)),
        }
    }
}

impl<T: QueryTarget> From<TargetSet<T>> for QueryValue<T> {
    fn from(v: TargetSet<T>) -> Self {
        QueryValue::TargetSet(v)
    }
}

impl<T: QueryTarget> From<FileSet> for QueryValue<T> {
    fn from(v: FileSet) -> Self {
        QueryValue::FileSet(v)
    }
}
