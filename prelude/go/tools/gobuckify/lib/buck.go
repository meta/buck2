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
	"fmt"
	"slices"
	"strings"
)

// OSDeps is a map of Buck-OS to ArchDeps
type OSDeps struct {
	OS       string
	ArchDeps map[string]*ArchDeps
}

// ArchDeps is a map of Buck-Arch to CGO-specific dependency sets
type ArchDeps struct {
	Arch       string
	CommonDeps *StringSet
	CgoDeps    map[string]*StringSet
}

// BuckTarget is a buck-friendly representation of a go package
type BuckTarget struct {
	Name                 string
	ImportPath           string
	IsBinary             bool
	EmbedFiles           StringSet
	CommonDeps           []string
	PlatformDeps         map[string]*OSDeps
	TargetCompatibleWith map[string]map[string]*StringSet // os => arch => cgo constraints
}

// Normalise prepares BuckTarget to be written to a file:
// - Moves common dependencies to CommonDeps out of PlatformDeps
// - Removes empty PlatformDeps
// - Removes TargetCompatibleWith if all configurations are compatible
func (b *BuckTarget) Normalise(totalConfigurationNumber int) {
	freqmap := make(map[string]int)
	for _, osDeps := range b.PlatformDeps {
		for _, archDeps := range osDeps.ArchDeps {
			for _, deps := range archDeps.CgoDeps {
				for dep := range *deps {
					freqmap[dep]++
				}
			}
		}
	}

	// If a dependency is used in all configurations, it can be moved to CommonDeps
	for dep, freq := range freqmap {
		if freq == totalConfigurationNumber {
			b.CommonDeps = append(b.CommonDeps, dep)
			for _, osDeps := range b.PlatformDeps {
				for _, archDeps := range osDeps.ArchDeps {
					for _, deps := range archDeps.CgoDeps {
						deps.Remove(dep)
					}
				}
			}
		}
	}

	// Dependencies used with and without cgo do not need to be repeated in the select.
	for _, osDeps := range b.PlatformDeps {
		for _, archDeps := range osDeps.ArchDeps {
			disabled, hasDisabled := archDeps.CgoDeps["prelude//go/constraints:cgo_enabled[false]"]
			enabled, hasEnabled := archDeps.CgoDeps["prelude//go/constraints:cgo_enabled[true]"]
			if !hasDisabled || !hasEnabled {
				continue
			}
			commonDeps := NewSet()
			for dep := range *disabled {
				if _, ok := (*enabled)[dep]; ok {
					commonDeps.Add(dep)
					disabled.Remove(dep)
					enabled.Remove(dep)
				}
			}
			if commonDeps.Len() > 0 {
				archDeps.CommonDeps = commonDeps
			}
		}
	}

	// Remove empty PlatformDeps
	for os, osDeps := range b.PlatformDeps {
		for arch, archDeps := range osDeps.ArchDeps {
			for cgo, deps := range archDeps.CgoDeps {
				if deps.Len() == 0 {
					delete(archDeps.CgoDeps, cgo)
				}
			}
			if archDeps.CommonDeps == nil && len(archDeps.CgoDeps) == 0 {
				delete(osDeps.ArchDeps, arch)
			}
		}
		if len(osDeps.ArchDeps) == 0 {
			delete(b.PlatformDeps, os)
		}
	}
	slices.Sort(b.CommonDeps)

	compatibleWithNumber := 0
	for _, archMap := range b.TargetCompatibleWith {
		for _, cgoConstraints := range archMap {
			compatibleWithNumber += cgoConstraints.Len()
		}
	}

	if compatibleWithNumber == totalConfigurationNumber {
		b.TargetCompatibleWith = nil // all configurations are compatible
	}
}

// BuckTargets is a map of buck targets keyed by import path
type BuckTargets map[string]*BuckTarget

// AddPackage adds a package to the buck targets map
func (b *BuckTargets) AddPackage(pkg *Package, buckOS, buckArch string, cgoEnabled bool) {
	// If a package with the same import path and configuration is added again, replace its data.
	var target *BuckTarget
	var ok bool
	if target, ok = (*b)[pkg.ImportPath]; !ok {
		target = &BuckTarget{
			Name:                 TargetNameFromImportPath(pkg.ImportPath),
			ImportPath:           pkg.ImportPath,
			PlatformDeps:         make(map[string]*OSDeps),
			EmbedFiles:           *NewSet(),
			IsBinary:             pkg.Name == "main",
			TargetCompatibleWith: make(map[string]map[string]*StringSet),
		}
		(*b)[pkg.ImportPath] = target
	}

	cgoConstraint := fmt.Sprintf("prelude//go/constraints:cgo_enabled[%t]", cgoEnabled)
	target.EmbedFiles.AddList(pkg.EmbedFiles)
	if target.TargetCompatibleWith[buckOS] == nil {
		target.TargetCompatibleWith[buckOS] = make(map[string]*StringSet)
	}
	if target.TargetCompatibleWith[buckOS][buckArch] == nil {
		target.TargetCompatibleWith[buckOS][buckArch] = NewSet()
	}
	target.TargetCompatibleWith[buckOS][buckArch].Add(cgoConstraint)

	if target.PlatformDeps[buckOS] == nil {
		target.PlatformDeps[buckOS] = &OSDeps{
			OS:       buckOS,
			ArchDeps: make(map[string]*ArchDeps),
		}
	}

	if target.PlatformDeps[buckOS].ArchDeps[buckArch] == nil {
		target.PlatformDeps[buckOS].ArchDeps[buckArch] = &ArchDeps{
			Arch:    buckArch,
			CgoDeps: make(map[string]*StringSet),
		}
	}
	target.PlatformDeps[buckOS].ArchDeps[buckArch].CgoDeps[cgoConstraint] = NewSet()

	for _, dep := range pkg.Imports {
		if !strings.ContainsRune(dep, '.') {
			continue // skip stdlib deps
		}
		target.PlatformDeps[buckOS].ArchDeps[buckArch].CgoDeps[cgoConstraint].Add(dep)
	}
}

func TargetNameFromImportPath(importPath string) string {
	lastSlash := strings.LastIndex(importPath, "/")
	var targetName string
	if lastSlash == -1 {
		targetName = importPath
	} else {
		targetName = importPath[lastSlash+1:]
	}
	return targetName
}

func TargetLabelFromImportPath(targetLabelPrefix, importPath string) string {
	return fmt.Sprintf("%s%s:%s", targetLabelPrefix, importPath, TargetNameFromImportPath(importPath))
}
