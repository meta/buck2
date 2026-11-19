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
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"
)

// analyzePackage runs the analyzer on a package holding a Go file and a .syso file
// and returns the JSON it wrote.
func analyzePackage(t *testing.T) map[string]any {
	t.Helper()
	dir := t.TempDir()
	for name, content := range map[string]string{"a.go": "package a\n", "impl.syso": ""} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	out := filepath.Join(dir, "out.json")
	// main registers its flags itself, so it needs a fresh flag set.
	flag.CommandLine = flag.NewFlagSet("pkg_analyzer", flag.ExitOnError)
	os.Args = []string{"pkg_analyzer", "-o", out, dir}
	main()
	data, err := os.ReadFile(out)
	if err != nil {
		t.Fatal(err)
	}
	var parsed map[string]any
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatal(err)
	}
	return parsed
}

// The analyzer lists .syso files like `go list` does.
func TestSysoFilesAreReported(t *testing.T) {
	parsed := analyzePackage(t)
	if got := parsed["GoFiles"]; len(got.([]any)) != 1 || got.([]any)[0] != "a.go" {
		t.Fatalf("GoFiles = %v", got)
	}
	got, ok := parsed["SysoFiles"]
	if !ok || len(got.([]any)) != 1 || got.([]any)[0] != "impl.syso" {
		t.Fatalf("SysoFiles = %v", got)
	}
}
