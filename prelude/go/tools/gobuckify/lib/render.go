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
	"log/slog"
	"os"
	"path/filepath"
	"sync"
	"text/template"
)

var TargetTemplate = `
{{- .Config.Preambule }}
{{- if .Target.IsBinary }}
{{- .Config.LoadGoBinaryRule}}
{{- else }}
{{- .Config.LoadGoLibraryRule}}
{{- end }}
{{- if .Target.IsBinary }}{{ .Config.GoBinaryRule}}{{ else }}{{ .Config.GoLibraryRule}}{{ end }}(
    name = "{{.Target.Name}}",
    package_name = "{{.Target.ImportPath}}",
    srcs = native.glob(["*.go", "*.s", "*.h", "*.c", "*.cc", "*.cpp", "*.S"]),
    header_namespace = "",
    {{- if or .Target.CommonDeps .Target.PlatformDeps }}
    {{.Config.DepsAttr}} = [
        {{- range .Target.CommonDeps }}
        {{- if $.Config.DepsTargetLabelPrefix }}
        "{{TargetLabelFromImportPath $.Config.DepsTargetLabelPrefix .}}",
        {{- else }}
        "{{.}}",
        {{- end }}
        {{- end }}
    ]{{ if .Target.PlatformDeps }} + select({
        "DEFAULT": [],
        {{- range $os, $osDeps := .Target.PlatformDeps }}
        "{{ $os }}": select({
            "DEFAULT": [],
            {{- range $arch, $archDeps := $osDeps.ArchDeps }}
            "{{ $arch }}": {{ if $archDeps.CommonDeps }}[
                {{- range $archDeps.CommonDeps.SortedList }}
                {{- if $.Config.DepsTargetLabelPrefix }}
                "{{TargetLabelFromImportPath $.Config.DepsTargetLabelPrefix .}}",
                {{- else }}
                "{{.}}",
                {{- end }}
                {{- end }}
            ]{{ if $archDeps.CgoDeps }} + {{ end }}{{ end }}{{ if $archDeps.CgoDeps }}select({
                "DEFAULT": [],
                {{- range $cgo, $deps := $archDeps.CgoDeps }}
                "{{ $cgo }}": [
                    {{- range $deps.SortedList }}
                    {{- if $.Config.DepsTargetLabelPrefix }}
                    "{{TargetLabelFromImportPath $.Config.DepsTargetLabelPrefix .}}",
                    {{- else }}
                    "{{.}}",
                    {{- end }}
                    {{- end }}
                ],
                {{- end }}
            }){{ end }},
            {{- end }}
        }),
        {{- end }}
    }){{ end }},
    {{- end }}
    {{- if and .Target.EmbedFiles .Config.GenerateEmbedSrcs }}
    embed_srcs = [
        {{- range .Target.EmbedFiles.SortedList }}
        "{{.}}",
        {{- end }}
    ],
    {{- end }}
    {{- if .Target.TargetCompatibleWith }}
    target_compatible_with = select({
        "DEFAULT": ["config//:none"],
        {{- range $os, $archMap := .Target.TargetCompatibleWith }}
        "{{ $os }}": select({
            "DEFAULT": ["config//:none"],
            {{- range $arch, $cgoConstraints := $archMap }}
            {{- if eq $cgoConstraints.Len 2 }}
            "{{ $arch }}": [],
            {{- else }}
            "{{ $arch }}": select({
                "DEFAULT": ["config//:none"],
                {{- range $cgoConstraints.SortedList }}
                "{{.}}": [],
                {{- end }}
            }),
            {{- end }}
            {{- end }}
        }),
        {{- end }}
    }),
    {{- end }}
    visibility = ["PUBLIC"],
)
`

type TemplateData struct {
	Config BuckConfig
	Target *BuckTarget
}

func RenderBuckFiles(cfg *Config, thirdPartyDir string, buckTargets BuckTargets) error {
	tmpl1 := template.New("buck")
	tmpl1.Funcs(template.FuncMap{
		"TargetLabelFromImportPath": TargetLabelFromImportPath,
	})
	tmpl1 = template.Must(tmpl1.Parse(TargetTemplate))

	errors := make(chan error)
	var wg sync.WaitGroup
	sem := make(chan struct{}, 50) // limit IO concurrency to some reasonable number
	for _, target := range buckTargets {
		wg.Go(func() {
			target.Normalise(2 * len(cfg.Platforms))

			// Acquire semaphore before doing IO
			sem <- struct{}{}
			defer func() { <-sem }()

			buckFile := filepath.Join(thirdPartyDir, "vendor", target.ImportPath, "BUCK")
			f, err := os.Create(buckFile)
			if err != nil {
				errors <- fmt.Errorf("can't create BUCK file: %w", err)
				return
			}
			defer f.Close()

			if err := tmpl1.Execute(f, TemplateData{Config: cfg.Buck, Target: target}); err != nil {
				errors <- fmt.Errorf("can't execute template: %w", err)
			}
		})
	}

	go func() {
		wg.Wait()
		close(errors)
	}()

	var lastErr error
	for err := range errors {
		slog.Error("RenderBuckFiles", "err", err)
		lastErr = err
	}

	return lastErr
}
