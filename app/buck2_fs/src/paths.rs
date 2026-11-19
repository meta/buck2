/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! The paths module for buck2.
//!
//! Introduces 'ForwardRelativePath', 'ForwardRelativePathBuf', 'AbsPath', and
//! 'AbsPathBuf', which are equivalents of 'Path' and 'PathBuf'.
//!
//! ForwardRelativePaths are fully normalized relative platform agnostic paths
//! that only points forward. This means  that there is no `.` or `..` in this
//! path, and does not begin with `/`. These are resolved to the 'PathBuf' by
//! resolving them against an 'AbsPath'.
//!
//! 'AbsPath' are absolute paths, meaning they must start with a directory root
//! of either `/` or some  windows root directory like `c:`. These behave
//! roughly like 'Path'.

pub mod abs_norm_path;
pub mod abs_path;
pub mod cmp_impls;
pub mod file_name;
pub mod fmt;
pub mod forward_rel_path;
pub mod intern_forward_rel_path;
pub mod into_filename_buf_iterator;
pub mod path_util;
pub mod relative_path;

pub use into_filename_buf_iterator::IntoFileNameBufIterator;

pub use self::relative_path::RelativePath;
pub use self::relative_path::RelativePathBuf;
