/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;

use buck2_error::conversion::rusqlite::Buck2ErrorAsRusqliteError;
use dupe::Dupe;
use rusqlite::ToSql;
use rusqlite::types::FromSql;
use rusqlite::types::FromSqlError;
use rusqlite::types::FromSqlResult;
use rusqlite::types::ToSqlOutput;
use rusqlite::types::ValueRef;

// Use 'static here to avoid rust-analyzer crash when pattern matching
// on these string literals. https://github.com/rust-lang/rust-analyzer/issues/20149
#[allow(clippy::redundant_static_lifetimes)]
pub(crate) const ARTIFACT_TYPE_DIRECTORY: &'static str = "directory";
#[allow(clippy::redundant_static_lifetimes)]
pub(crate) const ARTIFACT_TYPE_FILE: &'static str = "file";
#[allow(clippy::redundant_static_lifetimes)]
pub(crate) const ARTIFACT_TYPE_SYMLINK: &'static str = "symlink";
#[allow(clippy::redundant_static_lifetimes)]
pub(crate) const ARTIFACT_TYPE_EXTERNAL_SYMLINK: &'static str = "external_symlink";

#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactType {
    Directory,
    File,
    Symlink,
    ExternalSymlink,
}

impl Display for ArtifactType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.serialize())
    }
}

impl ArtifactType {
    fn serialize(self) -> &'static str {
        match self {
            ArtifactType::Directory => ARTIFACT_TYPE_DIRECTORY,
            ArtifactType::File => ARTIFACT_TYPE_FILE,
            ArtifactType::Symlink => ARTIFACT_TYPE_SYMLINK,
            ArtifactType::ExternalSymlink => ARTIFACT_TYPE_EXTERNAL_SYMLINK,
        }
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
#[error("Internal error: found unknown value '{}' for enum `artifact_type`", .0)]
struct UnknownArtifactTypeError(String);

impl FromSql for ArtifactType {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let value = value.as_str()?;
        match value {
            ARTIFACT_TYPE_DIRECTORY => Ok(ArtifactType::Directory),
            ARTIFACT_TYPE_FILE => Ok(ArtifactType::File),
            ARTIFACT_TYPE_SYMLINK => Ok(ArtifactType::Symlink),
            ARTIFACT_TYPE_EXTERNAL_SYMLINK => Ok(ArtifactType::ExternalSymlink),
            other => Err(FromSqlError::Other(Box::new(Buck2ErrorAsRusqliteError(
                UnknownArtifactTypeError(other.to_owned()).into(),
            )))),
        }
    }
}

impl ToSql for ArtifactType {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.serialize().to_sql()
    }
}
