/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package gobuckifylib

import (
	"os"
	"path/filepath"
	"testing"
)

// Only the root module's own packages are skipped.
func TestOnlyPackagesOfTheRootModuleAreSkipped(t *testing.T) {
	if isRootModulePackage("example.com/rootkit", "example.com/root") {
		t.Fatal("example.com/rootkit was skipped")
	}
	for _, p := range []string{"example.com/root", "example.com/root/sub"} {
		if !isRootModulePackage(p, "example.com/root") {
			t.Fatalf("%s was not skipped", p)
		}
	}
}

func writeGoMod(t *testing.T, content string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), "go.mod")
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

// The module name comes from the `module` directive, wherever it is and however
// it is quoted or commented.
func TestModuleNameIsTakenFromTheModuleDirective(t *testing.T) {
	for _, content := range []string{
		"// the root module\nmodule example.com/root\n",
		"module \"example.com/root\"\n",
		"module example.com/root // the root module\n",
		"\nmodule example.com/root\n",
		"module\texample.com/root\n",
	} {
		got, err := ReadModuleName(writeGoMod(t, content))
		if err != nil {
			t.Fatalf("%q: %v", content, err)
		}
		if got != "example.com/root" {
			t.Fatalf("%q: got %q", content, got)
		}
	}
	if _, err := ReadModuleName(writeGoMod(t, "go 1.24\n")); err == nil {
		t.Fatal("a go.mod without a module directive was accepted")
	}
}
