/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook

/**
 * Whether this compiler's Compose plugin writes `$stable` as a property whose backing field is
 * annotated `@JvmField`. From 2.4 it does (AbstractComposeLowering.buildStabilityField); 2.3 writes
 * a plain static field.
 */
internal const val COMPOSE_STABLE_FIELD_IS_JVM_FIELD = false

/**
 * Whether this compiler's Compose plugin calls every object stable before it looks at the object's
 * state. From 2.4 its StabilityInferencer answers Stable for any object first; 2.3 infers an
 * object's stability from its fields like any other class.
 */
internal const val COMPOSE_OBJECTS_ARE_STABLE = false

/**
 * Whether this compiler's Compose plugin calls a class that is not final of unknown stability, so
 * it writes no known-stable bit for it. From 2.4 its StabilityInferencer starts a non-final class
 * at `Stability.Unknown`; 2.3 infers it from its members like a final class. A subclass still takes
 * the members of such a superclass into account.
 */
internal const val COMPOSE_NON_FINAL_CLASSES_ARE_UNKNOWN = false

/**
 * Whether this compiler's Compose plugin calls a value class stable when one of its supertypes
 * carries a stable marker. From 2.5 it asks `hasStableMarkedDescendant`; 2.3 and 2.4 honour only
 * the value class's own marker, and otherwise take the stability of its underlying type.
 */
internal const val COMPOSE_VALUE_CLASS_MARKERS_INCLUDE_SUPERTYPES = false

/**
 * Whether this compiler's Compose plugin scopes class-stability inference by file. From 2.4 an
 * interface is `Unknown` wherever it is declared, and a public/internal superclass from another
 * file is forced to runtime stability instead of being analysed structurally. 2.3 analyses every
 * in-module class structurally, and calls only in-module interfaces `Unknown`: an external
 * interface takes the bitmask branch, and interfaces carry no bitmask, so it is `Unstable`.
 */
internal const val COMPOSE_STABILITY_IS_FILE_SCOPED = false
