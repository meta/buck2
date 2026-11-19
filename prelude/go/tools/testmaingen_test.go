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
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// generateMain runs the generator on a one-test package with the given flags and
// returns the generated main.go.
func generateMain(t *testing.T, flags ...string) string {
	t.Helper()
	dir := t.TempDir()
	testFile := filepath.Join(dir, "x_test.go")
	src := "package x\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {}\n"
	if err := os.WriteFile(testFile, []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	// The flag values live in package globals, so clear what an earlier run left.
	for k := range coverPkgs {
		delete(coverPkgs, k)
	}
	out := filepath.Join(dir, "main.go")
	os.Args = append([]string{"testmaingen", "--output", out}, flags...)
	os.Args = append(os.Args, testFile)
	main()
	data, err := os.ReadFile(out)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func coveredLine(mainGo string) string {
	for _, line := range strings.Split(mainGo, "\n") {
		if strings.Contains(line, "testdeps.Covered = ") {
			return strings.TrimSpace(line)
		}
	}
	return ""
}

// The cover packages come out sorted, so the same inputs always give the same file.
func TestCoverPackagesComeOutSorted(t *testing.T) {
	for i := 0; i < 40; i++ {
		got := coveredLine(generateMain(t, "--cover-mode=set", "--cover-pkgs=foo/c,foo/a,foo/b"))
		if got != `testdeps.Covered = " in foo/a, foo/b, foo/c"` {
			t.Fatalf("run %d: got %q", i, got)
		}
	}
}

// With a cover mode but no cover packages the coverage message names no packages.
func TestNoCoverPackagesNamesNone(t *testing.T) {
	if got := coveredLine(generateMain(t, "--cover-mode=set")); got != `testdeps.Covered = ""` {
		t.Fatalf("got %q", got)
	}
}
