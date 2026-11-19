/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package driver

import (
	"os"
	"path/filepath"
	"testing"
)

type filePlatform struct{}

func (filePlatform) ProjectDir() string { return "/" }

func (filePlatform) FileExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}

// A GOROOT, a stdlib file inside it, and a Buck-owned file in a sibling directory
// whose name starts with the GOROOT path.
func goRootFixture(t *testing.T) (goRoot string, stdFile string, buckFile string) {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	goRoot = filepath.Join(root, "usr", "local", "go")
	stdFile = filepath.Join(goRoot, "src", "fmt", "print.go")
	buckFile = filepath.Join(root, "usr", "local", "go-work", "x.go")
	for _, f := range []string{stdFile, buckFile} {
		if err := os.MkdirAll(filepath.Dir(f), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(f, []byte("package x\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return goRoot, stdFile, buckFile
}

func classifyFile(t *testing.T, file, goRoot string) *targetsByType {
	t.Helper()
	got, err := parsePatterns(filePlatform{}, []string{"file=" + file}, goRoot)
	if err != nil {
		t.Fatal(err)
	}
	return got
}

// Only a file inside GOROOT goes to the stdlib query; a sibling directory whose
// name starts with the GOROOT path, or an unknown GOROOT, keeps the file with Buck.
func TestOnlyFilesInsideGoRootGoToTheStdlibQuery(t *testing.T) {
	goRoot, stdFile, buckFile := goRootFixture(t)
	for _, root := range []string{goRoot, ""} {
		got := classifyFile(t, buckFile, root)
		if len(got.buckFiles) != 1 || len(got.stdFiles) != 0 {
			t.Fatalf("goRoot %q: stdFiles=%v buckFiles=%v", root, got.stdFiles, got.buckFiles)
		}
	}
	if got := classifyFile(t, stdFile, goRoot); len(got.stdFiles) != 1 || len(got.buckFiles) != 0 {
		t.Fatalf("stdlib file: stdFiles=%v buckFiles=%v", got.stdFiles, got.buckFiles)
	}
}
