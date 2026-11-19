/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.stub.container

import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub

class StubsContainerImpl : StubsContainer {
  private val stubs: MutableMap<String, KStub> = mutableMapOf()

  override fun find(pkg: String, className: String, innerClassNames: List<String>): KStub? {
    var outerStub: KStub = stubs["$pkg:$className"] ?: return null
    if (innerClassNames.isEmpty()) return outerStub

    // TODO: fix a non-optimal lookup
    innerClassNames.forEach { innerName ->
      outerStub = outerStub.innerStubs.find { it.name == innerName } ?: return null
    }

    return outerStub
  }

  override fun add(stub: KStub) {
    if (stubs.contains(stub.fqn)) {
      Logger.log("  [Error] Stubs container already contains ${stub.fqn}")
    }
    Logger.log(
        """
      |  [New stub]
      |    - name: ${stub.fqn}
      |    - type: ${stub.type.name}
    """
            .trimMargin(),
    )
    stubs[stub.fqn] = stub
  }

  override fun all(): Collection<KStub> = stubs.values
}

private val KStub.fqn: String
  get() = "$pkg:$name"
