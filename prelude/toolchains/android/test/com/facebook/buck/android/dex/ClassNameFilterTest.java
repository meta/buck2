/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.google.common.collect.ImmutableList;
import org.junit.Test;

public class ClassNameFilterTest {
  @Test
  public void testFiltering() {
    ClassNameFilter filter =
        ClassNameFilter.fromConfiguration(
            ImmutableList.of(
                "^org/acra/",
                "^org/tukaani/",
                "/FbInjector^",
                "^com/facebook/build/Config^",
                "/nodex/",
                // regex patterns
                "^-com\\/facebook\\/intent\\$(FbrpcIntent|ChooserActivityIntent)$",
                "^-^com\\/facebook\\/.*\\/util"));

    assertTrue(filter.matches("org/acra/Reporter"));
    assertTrue(filter.matches("org/tukaani/Decoder$State"));
    assertTrue(filter.matches("com/facebook/inject/FbInjector"));
    assertTrue(filter.matches("com/facebook/build/Config"));
    assertTrue(filter.matches("com/facebook/nodex/Splash"));
    assertFalse(filter.matches("borg/acra/Reporter"));
    assertFalse(filter.matches("worg/tukaani/Decoder"));
    assertFalse(filter.matches("com/facebook/inject/FbInjectorImpl"));
    assertFalse(filter.matches("com/facebook/inject/FbInjector^"));
    assertFalse(filter.matches("com/facebook/build/Configs"));
    assertFalse(filter.matches("dcom/facebook/build/Config"));
    assertFalse(filter.matches("com/facebook/fake/build/Config"));
    assertFalse(filter.matches("com/facebook/modex/Splash"));
    // Test cases for regex match
    assertTrue(filter.matches("com/facebook/intent$FbrpcIntent"));
    assertFalse(filter.matches("com/facebook/intent$FbrpcIntent/Config"));
    assertTrue(filter.matches("com/facebook/intent$ChooserActivityIntent"));
    assertFalse(filter.matches("/com/facebook/intent$ChooserActivityIntent"));
    assertTrue(filter.matches("com/facebook/intent/local/utility/store"));
    assertFalse(filter.matches("com/facebook/intent/local/store"));
    assertFalse(filter.matches("/com/facebook/whatever/util/whatever"));
  }
}
