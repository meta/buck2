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
	"fmt"
	"path/filepath"
	"strings"
)

type targetsByType struct {
	buckPatterns []string
	buckFiles    []string
	stdPatterns  []string
	stdFiles     []string
}

func parsePatterns(platform Platform, targets []string, goRoot string) (*targetsByType, error) {
	var fileTargets, pkgTargets []string
	for _, targ := range targets {
		targ = fixPattern(targ)

		// Hack for golangci-lint, it doesn't add the file= prefix, let's detect it ourselves
		// https://github.com/golangci/golangci-lint/blob/726b8153cac6c04238264b189f5b05ec059f9330/pkg/lint/package.go#L234-L250
		targetIsFile := filepath.Ext(targ) == ".go" && platform.FileExists(targ)
		if strings.HasPrefix(targ, "file=") || targetIsFile {
			fname := strings.TrimPrefix(targ, "file=")
			if !platform.FileExists(fname) {
				return nil, fmt.Errorf("cannot find input file '%s'", fname)
			}
			fname = absRealPath(fname)

			fileTargets = append(fileTargets, fname)
			continue
		}
		if strings.HasPrefix(targ, "pattern=") {
			pkgTargets = append(pkgTargets, strings.TrimPrefix(targ, "pattern="))
			continue
		}
		pkgTargets = append(pkgTargets, targ)
	}

	result := &targetsByType{}
	for _, pkg := range pkgTargets {
		if strings.Contains(pkg, "//") || strings.Contains(pkg, ":") {
			result.buckPatterns = append(result.buckPatterns, pkg)
		} else {
			result.stdPatterns = append(result.stdPatterns, pkg)
		}
	}

	for _, file := range fileTargets {
		if isUnderGoRoot(file, goRoot) {
			result.stdFiles = append(result.stdFiles, file)
		} else {
			result.buckFiles = append(result.buckFiles, file)
		}
	}

	return result, nil
}

// isUnderGoRoot reports whether file lies inside goRoot. An unknown GOROOT matches
// nothing, so Buck-owned files never go to the stdlib query by accident.
func isUnderGoRoot(file, goRoot string) bool {
	if goRoot == "" {
		return false
	}
	goRoot = filepath.Clean(goRoot)
	return file == goRoot || strings.HasPrefix(file, goRoot+string(filepath.Separator))
}

// fixPattern fixes target name
func fixPattern(target string) string {
	// Remove leading ./ added by golangci-lint
	// https://github.com/golangci/golangci-lint/blob/726b8153cac6c04238264b189f5b05ec059f9330/pkg/lint/package.go#L245
	return strings.TrimPrefix(target, fmt.Sprintf(".%c", filepath.Separator))
}

func absRealPath(f string) string {
	path, _ := filepath.Abs(f)
	path, _ = filepath.EvalSymlinks(path)
	return path
}
