---
id: toolchains
title: Toolchains
---

# Toolchains

Go toolchains in Buck2 come in two types: "regular" and "bootstrap".

- **Regular toolchains** are used to build Go code.
- **Bootstrap toolchains** are used to build the tools required to build Go
  code.

A minimal setup requires `:go`, `:go_bootstrap`, and `:python_bootstrap` in your
`toolchains//` cell.

Additionally, a `:cxx` toolchain is required:

- If you use CGo, you need a fully configured CXX toolchain
- If you don't use CGo, you can define it with a stub `no_toolchain` rule

Each type of Go toolchain has two implementations: "system" and "hermetic".

## Hermetic Toolchains (Recommended)

Hermetic toolchains are the recommended way to build Go code because they:

- Make builds reproducible
- Support all features of the Go rules

Hermetic toolchains consist of two rules: `go*_distr` and `go*_toolchain`.

The `*_distr` rules wrap Go distributions produced by `go tool dist`, such as
those available at [go.dev/dl](https://go.dev/dl/).

You can use either local or `http_archive` Go distributions.

### Toolchain Locations

- `@prelude//toolchains/go:go_toolchain.bzl`
- `@prelude//toolchains/go:go_bootstrap_toolchain.bzl`

### Example Configuration

Here's an example of `go_toolchain` using local Go distributions
(`go_bootstrap_toolchain` is defined similarly):

```python
load("@prelude//toolchains/go:go_toolchain.bzl", "go_distr", "go_toolchain")

# Note: selects are resolved against execution-platform
go_distr(
    name = "go_distr",
    go_os_arch = select({
        "config//os:linux": select({"config//cpu:x86_64": ("linux", "amd64")}),
        "config//os:macos": select({"config//cpu:arm64": ("darwin", "arm64")}),
    }),
    go_root = select({
        "config//os:linux": select({"config//cpu:x86_64": "path/to/go-linux-amd64"}),
        "config//os:macos": select({"config//cpu:arm64": "path/to/go-darwin-arm64"}),
    }),
    version = "1.X.Y",
)

# Note: selects are resolved against target-platform
go_toolchain(
    name = "go",
    env_go_arch = select({
        "config//cpu:arm64": "arm64",
        "config//cpu:x86_64": "amd64",
    }),
    env_go_os = select({
        "config//os:linux": "linux",
        "config//os:macos": "darwin",
        "config//os:windows": "windows",
    }),
    go_distr = ":go_distr",
    visibility = ["PUBLIC"],
)
```

For a complete example using `http_archive`, see
[examples/toolchains/go_toolchain](https://github.com/facebook/buck2/blob/main/examples/toolchains/go_toolchain/toolchains/BUCK).

### Advanced: Multi-platform Toolchains

If you support multiple execution platforms using standard Go distributions
(produced by `go tool dist`), you may encounter a situation where you have
multiple copies of the Go distribution with almost identical content.

The `go_distr` rule supports a non-standard layout of the Go distribution,
allowing you to maintain a single copy of the source code with `bin` and
`pkg/tool` directories containing binaries for all execution platforms you
support.

Set `multiplatform = True` on `go_distr` to enable this behavior. In this case,
the `bin` and `pkg/tool` directories must contain binaries in `goos_goarch`
subdirectories (built for the corresponding platforms).

See the example for a Go distribution supporting two execution platforms:
linux/amd64 and darwin/arm64.

```
third-party/go-toolchain/1.27.2
├── api
├── bin
│   ├── darwin_arm64
│   │   ├── go
│   │   └── gofmt
│   └── linux_amd64
│       └── ...
├── ...
├── pkg
│   ├── include
│   └── tool
│       ├── darwin_arm64
│       │   ├── asm
│       │   ├── cgo
│       │   └── ...
│       └── linux_amd64
│           └── ...
└── ...
```

You can produce it by downloading the `src` distribution from https://go.dev/dl/
and running the following commands (assuming you are running on `linux/amd64`
and have Go installed on your system).

```
$ cd distr_dir/src
$ CGO_ENABLED=0 GOOS=linux GOARCH=amd64 ./make.bash
$ CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 ./make.bash
$ mkdir ../bin/linux_amd64
$ mv ../bin/go ../bin/linux_amd64
$ mv ../bin/gofmt ../bin/linux_amd64
```

## System Toolchains

System toolchains are not intended for production builds but can be used for
examples and simple projects. Note that some features of Go rules might not work
with system toolchains, such as building ASM files.

### Toolchain Locations

- `@prelude//toolchains/go:system_go_toolchain.bzl`
- `@prelude//toolchains/go:system_go_bootstrap_toolchain.bzl`

### Example Configuration

```python
load("@prelude///toolchains/go:system_go_bootstrap_toolchain.bzl", "system_go_bootstrap_toolchain")
load("@prelude///toolchains/go:system_go_toolchain.bzl", "system_go_toolchain")

system_go_toolchain(
    name = "go",
    visibility = ["PUBLIC"],
)

system_go_bootstrap_toolchain(
    name = "go_bootstrap",
    visibility = ["PUBLIC"],
)
```
