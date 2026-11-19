/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package resources

import (
	"os"
	"testing"
)

func TestResource(t *testing.T) {
	out, err := os.ReadFile("testdata/input")
	if err != nil {
		t.Error("Could not read file 'testdata/input':", err)
	}

	if string(out) != "hello\n" {
		t.Errorf("Bad data read - expected 'hello\n', got %v", string(out))
	}
}
