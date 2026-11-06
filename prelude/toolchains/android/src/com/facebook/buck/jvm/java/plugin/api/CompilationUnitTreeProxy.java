/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.plugin.api;

import com.facebook.infer.annotation.Nullsafe;

/**
 * {@link com.sun.source.tree.CompilationUnitTree} is included with the compiler and is thus not
 * directly accessible from within Buck's class loader. This interface is used to identify objects
 * that serve as proxies for {@link com.sun.source.tree.CompilationUnitTree}'s within Buck's class
 * loader. These are translated back into the underlying {@link
 * com.sun.source.tree.CompilationUnitTree} when passed back into the compiler.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface CompilationUnitTreeProxy {}
