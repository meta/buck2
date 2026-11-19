/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package main

import (
	"strings"
	"testing"
)

func TestEmbeddedGreeting(t *testing.T) {
	if greeting == "" {
		t.Fatal("embedded greeting is empty")
	}
	trimmed := strings.TrimSpace(greeting)
	if trimmed != "Hello" {
		t.Errorf("expected greeting to be 'Hello', got %q", trimmed)
	}
}
