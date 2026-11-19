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
	"reflect"
	"slices"
	"testing"
)

func TestTargetNameFromImportPath(t *testing.T) {
	tests := []struct {
		name       string
		importPath string
		want       string
	}{
		{
			name:       "simple path",
			importPath: "github.com/example/pkg",
			want:       "pkg",
		},
		{
			name:       "no slashes",
			importPath: "my-lib.com",
			want:       "my-lib.com",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := TargetNameFromImportPath(tt.importPath)
			if got != tt.want {
				t.Errorf("TargetNameFromImportPath(%q) = %q, want %q", tt.importPath, got, tt.want)
			}
		})
	}
}

func TestTargetLabelFromImportPath(t *testing.T) {
	want := "//prefix/github.com/example/pkg:pkg"
	got := TargetLabelFromImportPath("//prefix/", "github.com/example/pkg")
	if got != want {
		t.Errorf("TargetLabelFromImportPath(%q, %q) = %q, want %q",
			"//prefix/", "github.com/example/pkg", got, want)
	}
}

func TestBuckTargetNormalise(t *testing.T) {
	targets := make(BuckTargets)
	for _, platform := range []struct {
		os      string
		imports []string
	}{
		{"linux", []string{"example.com/common/dep1", "example.com/common/dep2", "example.com/linux/dep"}},
		{"darwin", []string{"example.com/common/dep1", "example.com/common/dep2", "example.com/darwin/dep"}},
	} {
		for _, cgoEnabled := range []bool{false, true} {
			imports := slices.Clone(platform.imports)
			if cgoEnabled {
				imports = append(imports, "example.com/"+platform.os+"/cgo")
			}
			targets.AddPackage(&Package{
				ImportPath: "github.com/example/test",
				Imports:    imports,
			}, platform.os, "x86_64", cgoEnabled)
		}
	}

	target := targets["github.com/example/test"]
	target.Normalise(4)

	if !reflect.DeepEqual(target.CommonDeps, []string{"example.com/common/dep1", "example.com/common/dep2"}) {
		t.Errorf("CommonDeps = %v", target.CommonDeps)
	}
	if len(target.PlatformDeps) != 2 {
		t.Errorf("PlatformDeps length = %d, want 2", len(target.PlatformDeps))
	}
	for os, want := range map[string]string{
		"linux":  "example.com/linux/dep",
		"darwin": "example.com/darwin/dep",
	} {
		archDeps := target.PlatformDeps[os].ArchDeps["x86_64"]
		if archDeps.CommonDeps == nil {
			t.Errorf("%s common architecture deps = nil", os)
			continue
		}
		if got := archDeps.CommonDeps.SortedList(); !reflect.DeepEqual(got, []string{want}) {
			t.Errorf("%s common architecture deps = %v, want %v", os, got, []string{want})
		}
		wantCgo := "example.com/" + os + "/cgo"
		enabled := archDeps.CgoDeps["prelude//go/constraints:cgo_enabled[true]"]
		if enabled == nil {
			t.Errorf("%s CGO-enabled deps = nil", os)
			continue
		}
		if got := enabled.SortedList(); !reflect.DeepEqual(got, []string{wantCgo}) {
			t.Errorf("%s CGO deps = %v, want %v", os, got, []string{wantCgo})
		}
		if len(archDeps.CgoDeps) != 1 {
			t.Errorf("%s CGO dependency branches = %d, want 1", os, len(archDeps.CgoDeps))
		}
	}
	if target.TargetCompatibleWith != nil {
		t.Errorf("TargetCompatibleWith = %v, want nil", target.TargetCompatibleWith)
	}

	for _, cgoEnabled := range []bool{false, true} {
		targets.AddPackage(&Package{
			ImportPath: "github.com/example/empty",
		}, "linux", "x86_64", cgoEnabled)
	}
	emptyTarget := targets["github.com/example/empty"]
	emptyTarget.Normalise(2)
	if len(emptyTarget.PlatformDeps) != 0 {
		t.Errorf("PlatformDeps = %v, want none", emptyTarget.PlatformDeps)
	}
}

func TestBuckTargetsAddPackage(t *testing.T) {
	tests := []struct {
		name          string
		pkg           *Package
		buckOS        string
		buckArch      string
		wantName      string
		wantIsBinary  bool
		wantDepsCount int
	}{
		{
			name: "add library package",
			pkg: &Package{
				Name:       "testpkg",
				ImportPath: "github.com/example/testpkg",
				Imports:    []string{"github.com/example/dep1", "github.com/example/dep2", "fmt"},
				EmbedFiles: []string{"embed1.txt", "embed2.txt"},
			},
			buckOS:        "linux",
			buckArch:      "x86_64",
			wantName:      "testpkg",
			wantIsBinary:  false,
			wantDepsCount: 2, // Only non-stdlib deps
		},
		{
			name: "add binary package",
			pkg: &Package{
				Name:       "main",
				ImportPath: "github.com/example/cmd",
				Imports:    []string{"github.com/example/dep1", "fmt"},
				EmbedFiles: []string{},
			},
			buckOS:        "darwin",
			buckArch:      "arm64",
			wantName:      "cmd",
			wantIsBinary:  true,
			wantDepsCount: 1, // Only non-stdlib deps
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			targets := make(BuckTargets)
			targets.AddPackage(tt.pkg, tt.buckOS, tt.buckArch, true)

			// Check if the package was added
			target, exists := targets[tt.pkg.ImportPath]
			if !exists {
				t.Fatalf("Package not added to targets")
			}

			// Check basic properties
			if target.Name != tt.wantName {
				t.Errorf("Target name = %q, want %q", target.Name, tt.wantName)
			}

			if target.ImportPath != tt.pkg.ImportPath {
				t.Errorf("Target import path = %q, want %q", target.ImportPath, tt.pkg.ImportPath)
			}

			if target.IsBinary != tt.wantIsBinary {
				t.Errorf("Target IsBinary = %v, want %v", target.IsBinary, tt.wantIsBinary)
			}

			if !reflect.DeepEqual(target.EmbedFiles.SortedList(), tt.pkg.EmbedFiles) {
				t.Errorf("Target EmbedFiles = %v, want %v", target.EmbedFiles.SortedList(), tt.pkg.EmbedFiles)
			}

			// Check platform deps
			osDeps, exists := target.PlatformDeps[tt.buckOS]
			if !exists {
				t.Fatalf("OS deps not found for %s", tt.buckOS)
			}

			archDeps, exists := osDeps.ArchDeps[tt.buckArch]
			if !exists {
				t.Fatalf("Arch deps not found for %s", tt.buckArch)
			}

			deps, exists := archDeps.CgoDeps["prelude//go/constraints:cgo_enabled[true]"]
			if !exists {
				t.Fatal("CGO-enabled deps not found")
			}

			if deps.Len() != tt.wantDepsCount {
				t.Errorf("Deps count = %d, want %d", deps.Len(), tt.wantDepsCount)
			}

			// Check TargetCompatibleWith
			archMap, exists := target.TargetCompatibleWith[tt.buckOS]
			if !exists {
				t.Fatalf("TargetCompatibleWith not found for %s", tt.buckOS)
			}

			cgoConstraints, exists := archMap[tt.buckArch]
			if !exists {
				t.Fatalf("Arch %s not found in TargetCompatibleWith for OS %s", tt.buckArch, tt.buckOS)
			}
			if _, exists := (*cgoConstraints)["prelude//go/constraints:cgo_enabled[true]"]; !exists {
				t.Error("CGO-enabled constraint not found in TargetCompatibleWith")
			}
		})
	}
}

func TestAddPackageMultiplePlatforms(t *testing.T) {
	targets := make(BuckTargets)
	pkg := &Package{
		ImportPath: "github.com/example/testpkg",
		Imports:    []string{"github.com/example/dep1", "github.com/example/dep2"},
	}

	// Add the same package for multiple platforms
	targets.AddPackage(pkg, "linux", "x86_64", true)
	targets.AddPackage(pkg, "linux", "arm64", true)
	targets.AddPackage(pkg, "darwin", "x86_64", true)

	target, exists := targets[pkg.ImportPath]
	if !exists {
		t.Fatalf("Package not added to targets")
	}

	// Check that all platforms were added
	if len(target.TargetCompatibleWith) != 2 {
		t.Errorf("Expected 2 OS entries in TargetCompatibleWith, got %d", len(target.TargetCompatibleWith))
	}

	// Check linux architectures
	linuxArchs, exists := target.TargetCompatibleWith["linux"]
	if !exists {
		t.Fatalf("Linux not found in TargetCompatibleWith")
	}
	if len(linuxArchs) != 2 {
		t.Errorf("Expected 2 architectures for Linux, got %d", len(linuxArchs))
	}

	// Check darwin architectures
	darwinArchs, exists := target.TargetCompatibleWith["darwin"]
	if !exists {
		t.Fatalf("Darwin not found in TargetCompatibleWith")
	}
	if len(darwinArchs) != 1 {
		t.Errorf("Expected 1 architecture for Darwin, got %d", len(darwinArchs))
	}

	// Check platform deps
	if len(target.PlatformDeps) != 2 {
		t.Errorf("Expected 2 OS entries in PlatformDeps, got %d", len(target.PlatformDeps))
	}

	linuxDeps, exists := target.PlatformDeps["linux"]
	if !exists {
		t.Fatalf("Linux not found in PlatformDeps")
	}
	if len(linuxDeps.ArchDeps) != 2 {
		t.Errorf("Expected 2 architectures for Linux in PlatformDeps, got %d", len(linuxDeps.ArchDeps))
	}
}

func TestAddPackageMultipleCgoConfigurations(t *testing.T) {
	const importPath = "github.com/example/testpkg"
	targets := make(BuckTargets)
	targets.AddPackage(&Package{
		ImportPath: importPath,
		Imports:    []string{"github.com/example/common", "github.com/example/nocgo"},
	}, "linux", "x86_64", false)
	targets.AddPackage(&Package{
		ImportPath: importPath,
		Imports:    []string{"github.com/example/common", "github.com/example/cgo"},
	}, "linux", "x86_64", true)

	target := targets[importPath]
	target.Normalise(2)

	if !reflect.DeepEqual(target.CommonDeps, []string{"github.com/example/common"}) {
		t.Errorf("CommonDeps = %v", target.CommonDeps)
	}
	archDeps := target.PlatformDeps["linux"].ArchDeps["x86_64"]
	got := archDeps.CgoDeps["prelude//go/constraints:cgo_enabled[false]"].SortedList()
	if !reflect.DeepEqual(got, []string{"github.com/example/nocgo"}) {
		t.Errorf("CGO-disabled deps = %v", got)
	}
	got = archDeps.CgoDeps["prelude//go/constraints:cgo_enabled[true]"].SortedList()
	if !reflect.DeepEqual(got, []string{"github.com/example/cgo"}) {
		t.Errorf("CGO-enabled deps = %v", got)
	}
	if target.TargetCompatibleWith != nil {
		t.Errorf("TargetCompatibleWith = %v, want nil", target.TargetCompatibleWith)
	}
}

func TestAddCgoOnlyPackage(t *testing.T) {
	targets := make(BuckTargets)
	targets.AddPackage(&Package{
		ImportPath: "github.com/example/testpkg",
		Imports:    []string{"github.com/example/cgo"},
	}, "linux", "x86_64", true)

	target := targets["github.com/example/testpkg"]
	target.Normalise(2)

	if len(target.CommonDeps) != 0 {
		t.Errorf("CommonDeps = %v, want none", target.CommonDeps)
	}
	if target.TargetCompatibleWith == nil {
		t.Fatal("TargetCompatibleWith = nil, want CGO-enabled compatibility")
	}
	cgoConstraints := target.TargetCompatibleWith["linux"]["x86_64"]
	if got := cgoConstraints.SortedList(); !reflect.DeepEqual(got, []string{"prelude//go/constraints:cgo_enabled[true]"}) {
		t.Errorf("CGO constraints = %v", got)
	}
}
