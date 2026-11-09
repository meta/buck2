/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer.common;

import com.facebook.infer.annotation.Nullsafe;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.OptionDef;
import org.kohsuke.args4j.spi.OptionHandler;
import org.kohsuke.args4j.spi.Parameters;
import org.kohsuke.args4j.spi.Setter;

/** Consume all options handler for args4j library */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ConsumeAllOptionsHandler extends OptionHandler<String> {

  public ConsumeAllOptionsHandler(
      CmdLineParser parser, OptionDef option, Setter<? super String> setter) {
    super(parser, option, setter);
  }

  @Override
  public int parseArguments(Parameters params) throws CmdLineException {
    for (int idx = 0; idx < params.size(); idx += 1) {
      setter.addValue(params.getParameter(idx));
    }
    return params.size();
  }

  @Override
  public String getDefaultMetaVariable() {
    return "LIST<STRING>";
  }
}
