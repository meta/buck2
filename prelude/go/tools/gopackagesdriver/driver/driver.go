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
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"os"
	"runtime/debug"
	"strings"
	"time"

	"golang.org/x/tools/go/packages"
)

const envVarPkgDriver = "GOPACKAGESDRIVER"

const (
	// Special case targets for meta-packages
	metaAllPackages = "./..."
	metaAll         = "all"
	metaCmd         = "cmd"
	metaStdLib      = "std"
)

var allPackagesTargetExprs = strings.Fields(os.Getenv("GOPACKAGESDRIVER_BUCK_ALL_PACKAGES_TARGET_EXPRS"))

func query(ctx context.Context, req *packages.DriverRequest, bucker Bucker, platform Platform, targets []string) (*packages.DriverResponse, error) {
	var resp *packages.DriverResponse
	var err error

	// The LSP tries to do this on start, which is all packages
	// This will get stuck for a while so ignore it
	safeTargets := make([]string, 0, len(targets))
	for _, target := range targets {
		if target == metaAllPackages || target == metaAll {
			safeTargets = append(safeTargets, allPackagesTargetExprs...)
			continue
		}
		safeTargets = append(safeTargets, target)
	}

	goBinary, cleanup, err := getGoBinary()
	if err != nil {
		slog.Error("failed to get go binary", "err", err)
		return nil, err
	}
	defer cleanup()

	goRootDir, err := getGoRoot(goBinary)
	if err != nil {
		slog.Error("failed to get go root", "err", err)
	}

	targetsByType, err := parsePatterns(platform, safeTargets, goRootDir)
	if err != nil {
		return nil, err
	}

	resp, err = queryBXL(ctx, req, bucker, targetsByType.buckPatterns, targetsByType.buckFiles)
	if err != nil {
		slog.Error("error when query BXL", "args", os.Args, "err", err)
		return nil, err
	}

	if len(resp.Roots) > 0 {
		stdDeps, err := getImportedStdPackages(ctx, resp, req, goBinary)
		if err != nil {
			return nil, err
		}
		resp.Packages = append(resp.Packages, stdDeps...)
	}

	if len(targetsByType.stdFiles) > 0 || len(targetsByType.stdPatterns) > 0 {
		query := buildStdQuery(targetsByType.stdFiles, targetsByType.stdPatterns)
		stdPackages, err := queryStd(ctx, req, goBinary, query)
		if err != nil {
			return nil, err
		}

		roots := extractRoots(stdPackages)

		if req.Mode&packages.NeedDeps != 0 {
			stdPackages = flattenDeps(stdPackages)
		}

		resp.Roots = append(resp.Roots, roots...)
		resp.Packages = append(resp.Packages, stdPackages...)
	}

	// fallback to `go list` if we don't have any roots
	// this will allow us to resolve non-buck targets
	if len(resp.Roots) == 0 {
		resp.NotHandled = true
	}

	return resp, nil
}

func readDriverRequest() (*packages.DriverRequest, error) {
	stat, err := os.Stdin.Stat()
	if err != nil {
		return nil, fmt.Errorf("could not stat stdin: %w", err)
	}

	// No data being piped in
	if (stat.Mode() & os.ModeCharDevice) != 0 {
		slog.Info("no request data in stdin")
		return &packages.DriverRequest{}, nil
	}

	slog.Info("reading request data from stdin")
	reqData, err := io.ReadAll(os.Stdin)

	if err != nil {
		return nil, err
	}
	var req packages.DriverRequest
	if err := json.Unmarshal(reqData, &req); err != nil {
		return nil, fmt.Errorf("could not unmarshal driver request: %w", err)
	}

	if req.Overlay == nil {
		req.Overlay = map[string][]byte{}
	}

	return &req, nil
}

// RunOptions holds configuration options for the driver
type RunOptions struct {
	telemetry Telemetry
}

// Option is a functional option for configuring RunOptions
type Option func(*RunOptions)

// WithTelemetry sets the telemetry implementation
func WithTelemetry(t Telemetry) Option {
	return func(opts *RunOptions) {
		opts.telemetry = t
	}
}

// Run parses the command line arguments and stdin, then runs buck2 and `go list` and writes results to stdout
func Run(ctx context.Context, opts ...Option) error {
	options := &RunOptions{
		telemetry: &NoopTelemetry{},
	}
	for _, opt := range opts {
		opt(options)
	}

	telemetry := options.telemetry
	defer func() {
		if r := recover(); r != nil {
			telemetry.LogEvent(ctx, &PanicEvent{
				PanicValue: r,
				Stack:      debug.Stack(),
			})
			panic(r)
		}
	}()

	start := time.Now()

	// This must be cleared or we could forkbomb ourselves
	// because we will call GoListDriver inside.
	os.Setenv(envVarPkgDriver, "off")

	switch os.Getenv("GOPACKAGESDRIVER_LOG_LEVEL") {
	case "debug":
		slog.SetLogLoggerLevel(slog.LevelDebug)
	case "info":
		slog.SetLogLoggerLevel(slog.LevelInfo)
	case "warn":
		slog.SetLogLoggerLevel(slog.LevelWarn)
	case "error":
		slog.SetLogLoggerLevel(slog.LevelError)
	}

	cwd := CWD()
	targets := os.Args[1:]
	cmder := &shellCommander{}
	bucker := &buckShell{
		cmder:       cmder,
		buckOptions: strings.Fields(os.Getenv("GOPACKAGESDRIVER_BUCK_OPTIONS")),
	}

	req, err := readDriverRequest()
	if err != nil {
		return fmt.Errorf("failed to read driver request: %w", err)
	}

	platform, err := newPlatform(ctx, bucker, req)
	if err != nil {
		slog.Error("error creating platform", "err", err)
		return err
	}

	slog.Info("running with args", "cwd", cwd, "project", platform.ProjectDir(), "args", os.Args)
	slog.Debug("running with env", "env", os.Environ())

	var resp *packages.DriverResponse

	defer func() {
		telemetry.LogEvent(ctx, &RequestFinishedEvent{
			Duration: time.Since(start),
			BuckRoot: platform.ProjectDir(),
			Request:  req,
			Patterns: targets,
			Response: resp,
			Error:    err,
		})
	}()

	resp, err = query(ctx, req, bucker, platform, targets)
	if err != nil {
		return err
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent(" ", "    ")
	return enc.Encode(resp)
}
