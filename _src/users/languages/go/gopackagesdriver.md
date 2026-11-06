---
id: gopackagesdriver
title: IDE/Tools Integration (gopackagesdriver)
---

# IDE/Linter Integration

[The Go packages driver protocol](https://pkg.go.dev/golang.org/x/tools/go/packages)
is the standard way to integrate build systems with Go tools. Buck2 implements
this protocol using BXL API, enabling
[golang.org/x/tools/go/packages.Load()](https://pkg.go.dev/golang.org/x/tools/go/packages#Load)
based tools to work with Buck2.

## How to use it? (golangci-lint example)

Set the `GOPACKAGESDRIVER` environment variable to Buck2's gopackagesdriver
binary path and use your Go tool as usual.

There's no precise specification on how CLI tools should process input
arguments, but in general they should treat them as opaque. There are also query
operators `file=` and `pattern=`
[(docs)](https://pkg.go.dev/golang.org/x/tools/go/packages#pkg-overview:~:text=Two%20query%20operators%20are%20currently%20supported%3A%20%22file%22%20and%20%22pattern%22.).
So for a package `"foo/bar"`, one of the examples below should work.

Let's see a [`golangci-lint`](https://github.com/golangci/golangci-lint)
example:

```
$ export GOPACKAGESDRIVER=$(buck2 build prelude//go/tools/gopackagesdriver:gopackagesdriver --show-full-simple-output)
$ golangci-lint run foo/bar/baz.go
$ golangci-lint run file=foo/bar/baz.go # use file= if previous command doesn't work
$ golangci-lint run root//foo/bar:bar
$ golangci-lint run pattern=root//foo/bar:bar # use pattern= if previous command doesn't work
```

## IDE Integration (gopls/VSCode)

Here's an example of using
[gopls](https://github.com/golang/tools/tree/master/gopls) and VSCode with
Buck2. You can use the same approach for other IDEs and tools.

1. Install
   [Go VSCode extension](https://marketplace.visualstudio.com/items?itemName=golang.go).

2. Create a wrapper script `tools/bin/gopackagesdriver.sh` inside your repo.

```sh
#!/usr/bin/env bash
exec buck2 run prelude//go/tools/gopackagesdriver:gopackagesdriver -- "${@}"
```

3. Configure VSCode to use the driver.

```json
{
  "go.toolsEnvVars": {
    "GOPACKAGESDRIVER": "${workspaceFolder}/tools/bin/gopackagesdriver.sh", # (required) path to the driver
    "GOPACKAGESDRIVER_BUCK_OPTIONS": "--target-platforms prelude//platforms:default", # (optional) if your `toolchains//:go` requires it
    "GOPACKAGESDRIVER_BUCK_ALL_PACKAGES_TARGET_EXPRS": "root//..." # (optional) index all packages on gopls startup (might be slow and unreliable)
  },
  "gopls": {
    "build.workspaceFiles": [ # (required) to handle changes in BUCK files
      "**/BUCK",
      "**/PACKAGE",
      "**/*.bzl",
      "**/.buckconfig"
    ]
  }
}
```

## What tools are supported?

Theoretically, any tool based on
[`packages.Load()`](https://pkg.go.dev/golang.org/x/tools/go/packages#Load)
should work. However, in practice, some tools may have assumptions about
specific build systems.

There are
[8.2k occurrences](https://github.com/search?q=lang%3AGo+%22packages.Load%28%22&type=code)
of `packages.Load` on GitHub. You can check if your favorite tool is among them.
If it's not working correctly, you can make a PR to adjust its behavior.

The following tools are known to work well with `GOPACKAGESDRIVER`:
[gopls](https://github.com/golang/tools/tree/master/gopls),
[golangci-lint](https://github.com/golangci/golangci-lint),
[scip-go](https://github.com/scip-code/scip-go),
[callgraph](https://stackoverflow.com/questions/31362332/creating-call-graph/31369718#31369718).

## What tools are not supported?

Tools that directly call `go list` or `go build` are incompatible with this
approach.

## Configuration

The driver is configured via environment variables:

- `GOPACKAGESDRIVER_BUCK_OPTIONS` - options passed to `buck2 bxl` and
  `buck2 run` commands.
- `GOPACKAGESDRIVER_BUCK_ALL_PACKAGES_TARGET_EXPRS` - a list of target
  expressions separated by space, useful to replace `./...` query that `gopls`
  does on startup.
- `GOPACKAGESDRIVER_LOG_LEVEL` - log level, one of `debug`, `info`, `warn`,
  `error`. Defaults to `info`.
