/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub(crate) mod base;
mod fat;
mod packed;
mod slice;
mod string_like;
mod thin;
mod thin_slice;

pub use crate::arc_str::fat::ArcStr;
pub use crate::arc_str::packed::PackedStr;
pub use crate::arc_str::packed::PackedStringSlice;
pub use crate::arc_str::packed::PackedStringSliceBuilder;
pub use crate::arc_str::slice::ArcSlice;
pub use crate::arc_str::string_like::ArcS;
pub use crate::arc_str::string_like::StringInside;
pub use crate::arc_str::string_like::ThinArcS;
pub use crate::arc_str::thin::ThinArcStr;
pub use crate::arc_str::thin_slice::ThinArcSlice;
