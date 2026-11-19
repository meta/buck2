/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.command.kotlin

import com.facebook.buck.cd.model.kotlin.BuildCommand as ProtoBuildCommand
import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.jvm.cd.command.BaseJarCommand
import com.facebook.buck.jvm.cd.command.BuildMode
import com.facebook.buck.jvm.cd.serialization.kotlin.KotlinExtraParamsSerializer
import java.util.Optional

class BuildKotlinCommand(
    val kotlinExtraParams: KotlinExtraParams,
    val baseJarCommand: BaseJarCommand,
    val buildMode: BuildMode,
) {

  companion object {
    fun fromProto(model: ProtoBuildCommand, scratchDir: Optional<RelPath>): BuildKotlinCommand {

      val buildMode: BuildMode = BuildMode.fromProto(model.buildMode)
      val baseJarCommand: BaseJarCommand =
          BaseJarCommand.fromProto(model.baseJarCommand, scratchDir)
      val kotlinExtraParams: KotlinExtraParams =
          KotlinExtraParamsSerializer.deserialize(
              model.baseJarCommand.resolvedJavacOptions,
              model.kotlinExtraParams,
              scratchDir,
          )

      return BuildKotlinCommand(kotlinExtraParams, baseJarCommand, buildMode)
    }
  }
}
