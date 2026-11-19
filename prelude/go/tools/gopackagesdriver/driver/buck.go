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
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"regexp"
	"slices"
	"strings"
)

// Commander is an interface for execing commands and getting stdout results
type Commander interface {
	Exec(ctx context.Context, name string, args ...string) ([]byte, error)
}

type shellCommander struct{}

// Bucker is a mockable interface of the buck functionality this package uses
type Bucker interface {
	Root(ctx context.Context) (string, error)
	BXL(ctx context.Context, label string, args []string) ([]byte, error)
}

type buckShell struct {
	cmder       Commander
	buckOptions []string // common flags for run/build/test/bxl commands
}

func (s shellCommander) Exec(ctx context.Context, name string, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	data, err := cmd.Output()
	if err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			return nil, fmt.Errorf("failed to execute command '%v' (%w): %s", cmd.Args, ee, string(ee.Stderr))
		}
		return nil, fmt.Errorf("failed to execute command '%v', %w", cmd.Args, err)
	}
	return data, nil
}

var lineRe = regexp.MustCompile(`//line\s+(.+):1:1`)

// fixRePath replaces CGo line directives with the absolute source path reported
// by go/packages.
func fixRePath(file, absSrcPath string) error {
	content, err := os.ReadFile(file)
	if err != nil {
		return err
	}

	buf := new(bytes.Buffer)

	scanner := bufio.NewScanner(bytes.NewReader(content))
	for scanner.Scan() {
		origLine := scanner.Text()
		matches := lineRe.FindStringSubmatch(origLine)
		if len(matches) == 2 {
			srcPath := matches[1]
			slog.Debug("fixing CGo source path", "file", file, "old", srcPath, "new", absSrcPath)
			fmt.Fprintf(buf, "//line %s:1:1\n", absSrcPath)
		} else {
			buf.WriteString(origLine)
			buf.WriteString("\n")
		}
	}
	if err = scanner.Err(); err != nil {
		return err
	}
	dst, err := os.OpenFile(file, os.O_WRONLY|os.O_TRUNC, 0644)
	if err != nil {
		return err
	}
	defer dst.Close()
	_, err = io.Copy(dst, buf)
	return err
}

func (b *buckShell) run(ctx context.Context, args []string) ([]byte, error) {
	args = append([]string{"--client-metadata=id=gopackagesdriver"}, args...)
	slog.Debug("running 'buck2'", "args", args)
	out, err := b.cmder.Exec(ctx, "buck2", args...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (b *buckShell) Root(ctx context.Context) (string, error) {
	data, err := b.run(ctx, []string{"root", "-k", "project"})
	if err != nil {
		return "", err
	}

	return strings.TrimSpace(string(data)), nil
}

func (b *buckShell) BXL(ctx context.Context, label string, args []string) ([]byte, error) {
	data, err := b.run(ctx, slices.Concat([]string{"bxl"}, b.buckOptions, []string{label, "--"}, args))
	slog.Debug("running BXL", "label", label, "args", args)
	if err != nil {
		return nil, err
	}
	return data, nil
}

// Use this hacky way to get errors until https://fburl.com/workplace/q79a59rn implemented
func retriveActionErrors(buckStderr []byte) string {
	logPrefix := regexp.MustCompile(`^\[\S+\]\s*`)
	scanner := bufio.NewScanner(bytes.NewReader(buckStderr))
	sb := strings.Builder{}
	writtingStarted := false
	for scanner.Scan() {
		line := scanner.Text()

		if strings.Contains(line, "] BUILD ERRORS ") || strings.HasPrefix(line, "Traceback (most recent call last):") {
			writtingStarted = true
		}

		if line == "BXL FAILED" {
			break
		}

		if !writtingStarted {
			continue
		}

		if strings.Contains(line, "] The following actions failed") ||
			strings.Contains(line, "] Remote action, reproduce with:") ||
			strings.Contains(line, "] Local command") {
			continue
		}

		fmt.Fprintln(&sb, logPrefix.ReplaceAllString(line, ""))
	}

	if err := scanner.Err(); err != nil {
		return ""
	}

	return sb.String()
}
