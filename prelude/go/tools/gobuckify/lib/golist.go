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
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"regexp"
	"slices"
	"strings"
	"sync"
)

// moduleDirective matches the `module` directive of a go.mod file, whose module
// path may be double-quoted.
var moduleDirective = regexp.MustCompile(`^\s*module\s+"?([^\s"]+)"?`)

// Module represents a Go module from go list output
type Module struct {
	Path    string `json:"Path"`
	Version string `json:"Version"`
}

// Package is a subset of the fields of the golist output
type Package struct {
	Name       string
	ImportPath string
	Imports    []string
	EmbedFiles []string
	Standard   bool
	Module     *Module
}

func QueryGoList(
	workDir, rootModuleName, goOS, goArch string,
	cgoEnabled bool,
	extraArgs ...string,
) (chan *Package, chan error) {
	pkgChan := make(chan *Package, 1000) // 1000 is a guess, but should be enough
	errChan := make(chan error, 1)
	go func() {
		defer close(pkgChan)
		defer close(errChan)
		cmdArgs := slices.Concat(
			[]string{"buck2", "run", "toolchains//:go[go]", "--", "list", "-C", workDir, "-e", "-json=Name,ImportPath,Imports,EmbedFiles,Standard,Module"},
			extraArgs,
			[]string{"all"},
		)
		cmd := exec.Command(cmdArgs[0], cmdArgs[1:]...)
		// Set GOOS/GOARCH so `go list` applies each target platform's build constraints:
		// filename suffixes (e.g. `*_darwin.go`) and `//go:build` expressions (e.g.
		// `!linux`). Passing the platform only via `-tags` is insufficient -- the host's
		// own GOOS/GOARCH tags stay set, so negative and file-suffix constraints are
		// mis-evaluated and platform-specific deps get silently dropped.
		cgoEnabledValue := "0"
		if cgoEnabled {
			cgoEnabledValue = "1"
		}
		cmd.Env = append(os.Environ(), "CGO_ENABLED="+cgoEnabledValue, "GOOS="+goOS, "GOARCH="+goArch)

		stdout, err := cmd.StdoutPipe()
		if err != nil {
			errChan <- fmt.Errorf("failed to get stdout pipe: %w", err)
			return
		}

		stderr, err := cmd.StderrPipe()
		if err != nil {
			errChan <- fmt.Errorf("failed to get stderr pipe: %w", err)
			return
		}

		if err := cmd.Start(); err != nil {
			errChan <- fmt.Errorf("failed to start go list: %w", err)
			return
		}

		for dec := json.NewDecoder(stdout); dec.More(); {
			var pkg Package
			if err := dec.Decode(&pkg); err != nil {
				errChan <- fmt.Errorf("failed to decode json: %w", err)
				return
			}
			if pkg.Standard {
				continue // skip standard library packages
			}
			if pkg.Module == nil {
				continue // that's not a ligit third-party package
			}
			if isRootModulePackage(pkg.ImportPath, rootModuleName) {
				continue
			}
			pkgChan <- &pkg
		}

		stderrBytes, err := io.ReadAll(stderr)
		if err != nil {
			errChan <- fmt.Errorf("failed to read stderr: %w", err)
			return
		}

		if err := cmd.Wait(); err != nil {
			fmt.Fprintln(os.Stderr, string(stderrBytes))
			errChan <- fmt.Errorf("failed to wait for go list: %w", err)
		}
	}()

	return pkgChan, errChan
}

// isRootModulePackage reports whether importPath belongs to the root module, whose
// packages get no generated BUCK files.
func isRootModulePackage(importPath, rootModuleName string) bool {
	return importPath == rootModuleName || strings.HasPrefix(importPath, rootModuleName+"/")
}

// ReadModuleName returns the module path declared by the `module` directive of the
// go.mod file at path.
func ReadModuleName(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", fmt.Errorf("failed to open %s: %w", path, err)
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line, _, _ := strings.Cut(scanner.Text(), "//")
		if m := moduleDirective.FindStringSubmatch(line); m != nil {
			return m[1], nil
		}
	}
	if err := scanner.Err(); err != nil {
		return "", fmt.Errorf("failed to read %s: %w", path, err)
	}
	return "", fmt.Errorf("no module directive in %s", path)
}

// CollectPackagesResult contains the results of collecting packages from go list
type CollectPackagesResult struct {
	BuckTargets BuckTargets
	Modules     map[string]*Module
}

// CollectPackages queries go list for all platform and CGO configurations and
// collects packages into BuckTargets and unique Modules.
func CollectPackages(cfg *Config, thirdPartyDir, rootModuleName string) (*CollectPackagesResult, error) {
	type result struct {
		pkg        *Package
		buckOS     string
		buckArch   string
		cgoEnabled bool
	}

	results := make(chan *result, 2*1000*len(cfg.Platforms))
	mainErrChan := make(chan error)

	// Limit concurrency to avoid OOMs as `go list` can use a lot of memory
	semaphore := make(chan struct{}, 10)

	var wg sync.WaitGroup
	for _, p := range cfg.Platforms {
		for _, cgoEnabled := range []bool{false, true} {
			wg.Go(func() {
				semaphore <- struct{}{}
				defer func() { <-semaphore }()

				// GOOS/GOARCH and CGO_ENABLED are passed via the environment (see
				// QueryGoList), so `go list` evaluates each configuration correctly.
				pkgChan, errChan := QueryGoList(
					thirdPartyDir, rootModuleName, p.GoOS, p.GoArch, cgoEnabled,
					fmt.Sprintf("-tags=%s", strings.Join(cfg.DefaultTags, ",")),
				)
				pkgCount := 0
				for pkg := range pkgChan {
					pkgCount++
					results <- &result{pkg: pkg, buckOS: p.BuckOS, buckArch: p.BuckArch, cgoEnabled: cgoEnabled}
				}
				slog.Info("Found packages", "count", pkgCount, "os", p.GoOS, "arch", p.GoArch, "cgo_enabled", cgoEnabled)
				for err := range errChan {
					mainErrChan <- fmt.Errorf("error querying golist for %v with cgo_enabled=%t: %w", p, cgoEnabled, err)
				}
			})
		}
	}

	go func() {
		wg.Wait()
		close(results)
		close(mainErrChan)
	}()

	buckTargets := make(BuckTargets)
	modules := make(map[string]*Module)

	resultsClosed, mainErrChanClosed, hadErrors := false, false, false
	for {
		if resultsClosed && mainErrChanClosed {
			break
		}
		select {
		case res, ok := <-results:
			if !ok {
				resultsClosed = true
				continue
			}
			buckTargets.AddPackage(res.pkg, res.buckOS, res.buckArch, res.cgoEnabled)
			if res.pkg.Module != nil {
				modules[res.pkg.Module.Path] = res.pkg.Module
			}
		case err, ok := <-mainErrChan:
			if !ok {
				mainErrChanClosed = true
				continue
			}
			slog.Error("Error querying golist", "err", err)
			hadErrors = true
		}
	}

	if hadErrors {
		return nil, fmt.Errorf("errors occurred while collecting packages")
	}

	slog.Info("Packages collected", "count", len(buckTargets))
	slog.Info("Modules collected", "count", len(modules))

	return &CollectPackagesResult{
		BuckTargets: buckTargets,
		Modules:     modules,
	}, nil
}
