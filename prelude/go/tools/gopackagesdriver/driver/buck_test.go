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
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"golang.org/x/tools/go/packages"
)

// requireNoError fails the test if err is not nil
func requireNoError(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}
}

// requireEqual fails the test if expected != actual
func requireEqual(t *testing.T, expected, actual any) {
	t.Helper()
	if expected != actual {
		t.Fatalf("Expected %v, got %v", expected, actual)
	}
}

type mockCommander struct {
	output []byte
	err    error
	cmd    string
	args   []string
}

func (m *mockCommander) Exec(ctx context.Context, name string, args ...string) ([]byte, error) {
	m.cmd = name
	m.args = args
	return m.output, m.err
}

type mockCommanderChain struct {
	commands []*mockCommander
	pos      int
}

func (m *mockCommanderChain) Exec(ctx context.Context, name string, args ...string) ([]byte, error) {
	res, err := m.commands[m.pos].Exec(ctx, name, args...)
	m.pos++
	return res, err
}

func TestBuckRoot(t *testing.T) {
	ctx := context.Background()
	cmder := &mockCommander{output: []byte("\n/data/users/user1/fbcode\n"), err: nil}
	bucker := buckShell{cmder: cmder}

	root, err := bucker.Root(ctx)
	requireNoError(t, err)
	requireEqual(t, "/data/users/user1/fbcode", root)
}

func TestFixRePathTruncatesFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "generated.go")
	original := "//line ../../../../long/path/to/source.go:1:1\npackage example\n"
	requireNoError(t, os.WriteFile(path, []byte(original), 0644))

	requireNoError(t, fixRePath(path, "/src/source.go"))

	got, err := os.ReadFile(path)
	requireNoError(t, err)
	want := "//line /src/source.go:1:1\npackage example\n"
	requireEqual(t, want, string(got))
}

func TestCgoPathRewrites(t *testing.T) {
	testCases := []struct {
		name    string
		pkg     *packages.Package
		want    map[string]string
		wantErr string
	}{
		{
			name: "checked-in CGo source",
			pkg: &packages.Package{
				ID:              "cgo",
				GoFiles:         []string{"/repo/plain.go", "/repo/cgo_file.go"},
				CompiledGoFiles: []string{"/buck-out/cgo_gen/cgo_file.cgo1.go"},
			},
			want: map[string]string{
				"/buck-out/cgo_gen/cgo_file.cgo1.go": "/repo/cgo_file.go",
			},
		},
		{
			name: "generated SWIG source",
			pkg: &packages.Package{
				ID:              "swig",
				GoFiles:         []string{"/buck-out/swig/out/swig_lib.go"},
				CompiledGoFiles: []string{"/buck-out/cgo_gen/swig_lib.cgo1.go"},
			},
			want: map[string]string{
				"/buck-out/cgo_gen/swig_lib.cgo1.go": "/buck-out/swig/out/swig_lib.go",
			},
		},
		{
			name: "ignore CGo types file",
			pkg: &packages.Package{
				ID:              "cgo",
				CompiledGoFiles: []string{"/buck-out/cgo_gen/_cgo_gotypes.go"},
			},
		},
		{
			name: "missing source",
			pkg: &packages.Package{
				ID:              "cgo",
				GoFiles:         []string{"/repo/other.go"},
				CompiledGoFiles: []string{"/buck-out/cgo_gen/cgo_file.cgo1.go"},
			},
			wantErr: "found 0",
		},
		{
			name: "ambiguous source",
			pkg: &packages.Package{
				ID:              "cgo",
				GoFiles:         []string{"/repo/one/cgo_file.go", "/repo/two/cgo_file.go"},
				CompiledGoFiles: []string{"/buck-out/cgo_gen/cgo_file.cgo1.go"},
			},
			wantErr: "found 2",
		},
		{
			name: "relative source",
			pkg: &packages.Package{
				ID:              "cgo",
				GoFiles:         []string{"cgo_file.go"},
				CompiledGoFiles: []string{"/buck-out/cgo_gen/cgo_file.cgo1.go"},
			},
			wantErr: "is not absolute",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			got, err := cgoPathRewrites(tc.pkg)
			if tc.wantErr != "" {
				if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
					t.Fatalf("Expected error containing %q, got %v", tc.wantErr, err)
				}
				return
			}
			requireNoError(t, err)
			requireEqual(t, true, maps.Equal(tc.want, got))
		})
	}
}

func TestBuildBXLArgsRequestsGoFilesForCompiledGoFiles(t *testing.T) {
	req := &packages.DriverRequest{Mode: packages.NeedCompiledGoFiles}
	args := buildBXLArgs(req, nil, nil)
	requireEqual(t, true, slices.Contains(args, "--need_files"))
	requireEqual(t, true, slices.Contains(args, "--need_compiled_go_files"))
}

func TestFixQuery(t *testing.T) {
	testCases := []struct {
		name   string
		target string
		want   string
	}{
		{
			name:   "empty",
			target: "",
			want:   "",
		},
		{
			name:   "basic",
			target: "./fbcode//dns/lib/dnsgen/...",
			want:   "fbcode//dns/lib/dnsgen/...",
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			got := fixPattern(tc.target)
			requireEqual(t, tc.want, got)
		})
	}
}

func TestRetriveActionErrors(t *testing.T) {
	buckStderr := []byte(`[2024-09-12T07:57:42.799-07:00] Buck UI: https://www.internalfb.com/buck2/86f93efa-c28e-4a34-94a8-129da2fc5336
[2024-09-12T07:57:42.799-07:00] RE Session: reSessionID-21f215ea-c9f9-47db-a8d5-93787b9c3c7b
[2024-09-12T07:57:42.880-07:00] Action failed: fbcode//scripts/user1/helloworld/greeting:greeting (cfg:linux-x86_64-fbcode-platform010-clang17-asan-ubsan-dev#0b9c1ba57ab15e90) (go_compile greeting)
[2024-09-12T07:57:42.880-07:00] Local command returned non-zero exit code 2
[2024-09-12T07:57:42.880-07:00] Local command: env -- 'BUCK_SCRATCH_PATH=buck-out/v2/tmp/fbcode/638f78105dd4e4ee/go_compile/greeting' 'CGO_ENABLED=1' 'GOARCH=amd64' 'GOEXPERIMENT=nocoverageredesign' 'GOOS=linux' 'GOROOT=third-party/go-toolchain/1.23.1/linux_amd64' buck-out/v2/gen/prelude/a32dc125c1d570bc/go_bootstrap/tools/__go_go_wrapper__/go_go_wrapper --go buck-out/v2/gen/fbsource/a32dc125c1d570bc/third-party/go-toolchain/1.23.1/linux_amd64/__compile.exe__/compile.exe -- '-buildid=' -nolocalimports -trimpath '%cwd%' -p scripts/peters95/helloworld/greeting -importcfg buck-out/v2/gen/fbcode/0b9c1ba57ab15e90/scripts/peters95/helloworld/greeting/__greeting__/__action___0__/scripts/peters95/helloworld/greeting.final.importcfg -o buck-out/v2/gen/fbcode/0b9c1ba57ab15e90/scripts/peters95/helloworld/greeting/__greeting__/__action___0__/go_compile_out.a @buck-out/v2/gen/fbcode/0b9c1ba57ab15e90/scripts/peters95/helloworld/greeting/__greeting__/__action___0__/greeting_srcs.go_package_argsfile
[2024-09-12T07:57:42.880-07:00] Stdout:
fbcode/scripts/user1/helloworld/greeting/greeting.go:8:18: syntax error: unexpected name get, expected (
fbcode/scripts/user1/helloworld/greeting/greeting.go:10:1: syntax error: unexpected } after top level declaration
[2024-09-12T07:57:42.880-07:00] Stderr:
Error running command: exit status 2

/data/users/user1/fbsource/buck-out/v2/gen-bxl/fbcode/6788fe67763f3975/tools/go/driver/driver.bxl/__driver__e0355e856f017750__/dynamic_out.json
[2024-09-12T07:57:42.882-07:00] Cache hits: 0%
[2024-09-12T07:57:42.882-07:00] Commands: 1 (cached: 0, remote: 0, local: 1)
[2024-09-12T07:57:42.882-07:00] Network: Up: 0B  Down: 0B  (reSessionID-21f215ea-c9f9-47db-a8d5-93787b9c3c7b)
[2024-09-12T07:57:42.886-07:00]
[2024-09-12T07:57:42.886-07:00] BUILD ERRORS (1)
[2024-09-12T07:57:42.886-07:00] The following actions failed during the execution of this command:
[2024-09-12T07:57:42.886-07:00] Action failed: fbcode//scripts/user1/helloworld/greeting:greeting (cfg:linux-x86_64-fbcode-platform010-clang17-asan-ubsan-dev#0b9c1ba57ab15e90) (go_compile greeting)
[2024-09-12T07:57:42.886-07:00] Local command returned non-zero exit code 2
[2024-09-12T07:57:42.886-07:00] Local command: env -- 'BUCK_SCRATCH_PATH=buck-out/v2/tmp/fbcode/638f78105dd4e4ee/go_compile/greeting' 'CGO_ENABLED=1' 'GOARCH=amd64' 'GOEXPERIMENT=nocoverageredesign' 'GOOS=linux' 'GOROOT=third-party/go-toolchain/1.23.1/linux_amd64' buck-out/v2/gen/prelude/a32dc125c1d570bc/go_bootstrap/tools/__go_go_wrapper__/go_go_wrapper --go buck-out/v2/gen/fbsource/a32dc125c1d570bc/third-party/go-toolchain/1.23.1/linux_amd64/__compile.exe__/compile.exe -- '-buildid=' -nolocalimports -trimpath '%cwd%' -p scripts/peters95/helloworld/greeting -importcfg buck-out/v2/gen/fbcode/0b9c1ba57ab15e90/scripts/peters95/helloworld/greeting/__greeting__/__action___0__/scripts/peters95/helloworld/greeting.final.importcfg -o buck-out/v2/gen/fbcode/0b9c1ba57ab15e90/scripts/peters95/helloworld/greeting/__greeting__/__action___0__/go_compile_out.a @buck-out/v2/gen/fbcode/0b9c1ba57ab15e90/scripts/peters95/helloworld/greeting/__greeting__/__action___0__/greeting_srcs.go_package_argsfile
[2024-09-12T07:57:42.886-07:00] Stdout:
fbcode/scripts/user1/helloworld/greeting/greeting.go:8:18: syntax error: unexpected name get, expected (
fbcode/scripts/user1/helloworld/greeting/greeting.go:10:1: syntax error: unexpected } after top level declaration
[2024-09-12T07:57:42.886-07:00] Stderr:
Error running command: exit status 2

[2024-09-12T07:57:42.886-07:00]
BXL FAILED
Failed to build 'fbcode//scripts/user1/helloworld/greeting:greeting (cfg:linux-x86_64-fbcode-platform010-clang17-asan-ubsan-dev#0b9c1ba57ab15e90)'`)

	expected := `BUILD ERRORS (1)
Action failed: fbcode//scripts/user1/helloworld/greeting:greeting (cfg:linux-x86_64-fbcode-platform010-clang17-asan-ubsan-dev#0b9c1ba57ab15e90) (go_compile greeting)
Stdout:
fbcode/scripts/user1/helloworld/greeting/greeting.go:8:18: syntax error: unexpected name get, expected (
fbcode/scripts/user1/helloworld/greeting/greeting.go:10:1: syntax error: unexpected } after top level declaration
Stderr:
Error running command: exit status 2


`
	requireEqual(t, expected, retriveActionErrors(buckStderr))
}
