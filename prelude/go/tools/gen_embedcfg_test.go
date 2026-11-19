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
	"reflect"
	"strings"
	"testing"
)

// A package directory whose embedded tree holds a nested module and a VCS directory.
func embedFixture(t *testing.T) string {
	t.Helper()
	pkgdir := t.TempDir()
	for _, f := range []string{"static/a.txt", "static/mod/go.mod", "static/mod/m.txt", "static/.git/config"} {
		p := filepath.Join(pkgdir, filepath.FromSlash(f))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(f), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return pkgdir
}

// A nested module is skipped, and naming a VCS directory is an error, as in `go build`.
func TestNestedModuleIsSkippedAndNamedVcsDirectoryIsRejected(t *testing.T) {
	pkgdir := embedFixture(t)
	files, _, err := resolveEmbed(pkgdir, []string{"static"})
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"static/a.txt"}; !reflect.DeepEqual(files, want) {
		t.Fatalf("static: got %v, want %v", files, want)
	}
	_, _, err = resolveEmbed(pkgdir, []string{"static/.git"})
	if err == nil || !strings.Contains(err.Error(), "invalid name .git") {
		t.Fatalf("static/.git: got %v", err)
	}
}
