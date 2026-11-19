# AGENTS.md - VSGO (Visual Studio GO)

## Overview

VSGO is a project generator that produces Visual Studio solution (`.sln`) and project (`.vcxproj`) files from Buck2 C++ build targets. It enables developers to use Visual Studio's full IDE experience (IntelliSense, build, run, debug) on top of a Buck2-managed codebase without maintaining MSBuild project files manually.

**Owner team:** `windows_devx`

## Architecture

The system has two layers:

1. **Python CLI** (`vsgo.py`) - orchestrates the workflow by invoking multiple BXL scripts via `buck2 bxl`
2. **BXL scripts** (`.bxl` files) - query the Buck2 build graph, extract target attributes, resolve compiler/linker flags, and generate XML project files

Build/clean/rebuild in generated projects delegate to `buck2` via `msvs/vs_buck_build.props`, so compilation is always done by Buck2 — Visual Studio acts purely as an IDE front-end.

## Workflow

1. User invokes `vsgo.py` with Buck target(s) and mode file(s)
2. `gen_mode_configs.bxl` generates per-mode `.props` files
3. `get_mode_hashes.bxl` computes configuration hashes for path translation
4. `main.bxl` resolves targets, walks the dependency tree, extracts attributes, and generates `.vcxproj`, `.vcxproj.filters`, and `.sln` files
5. `vsgo.py` copies generated files to the user-specified `generated_projects/` folder

## Project Structure

### Entry Points

| File | Role |
|------|------|
| `vsgo.py` | Python CLI entry point. Parses arguments, orchestrates BXL invocations, copies outputs. |
| `main.bxl` | Primary BXL entry point. Resolves targets, walks deps, coordinates file generation. |

### Generators (BXL)

| File | Role |
|------|------|
| `gen_vcxproj.bxl` | Generates `.vcxproj` files with project configs, sources, headers, compiler/linker settings. |
| `gen_sln.bxl` | Generates `.sln` files with project references organized by cell/package hierarchy. |
| `gen_filters.bxl` | Generates `.vcxproj.filters` files for Solution Explorer virtual folder grouping. |
| `gen_mode_configs.bxl` | Generates per-mode MSBuild `.props` files for configuration switching. |
| `gen_user_macros.bxl` | Emits MSBuild PropertyGroup entries for RepoRoot, BuckTarget, debug settings, etc. |

### Data Extraction (BXL)

| File | Role |
|------|------|
| `get_attrs.bxl` | Extracts target attributes (sources, headers, flags). Handles platform-specific attrs. |
| `get_vs_settings.bxl` | Assembles Visual Studio settings by aggregating compiler/linker settings from transitive deps. |
| `get_deps.bxl` | Resolves dependency tree via `cquery().deps()` with DFS post-order traversal. |
| `get_compiler_settings.bxl` | Parses compiler flags into VS-compatible settings (defines, includes, warnings, etc.). |
| `get_linker_settings.bxl` | Parses linker flags into library directories, dependencies, and options. |
| `get_mode_hashes.bxl` | Computes Buck output hash per mode file for path translation. |

### Utilities

| File | Role |
|------|------|
| `utils.bxl` | XML generation (`h()`), GUID generation, path manipulation, logging. |
| `constants.bxl` | Shared constants: debug property names, toolset identifiers, mode keys. |
| `flags_parser_utils.bxl` | Parses compiler/linker flag strings (quoted args, `/D`, `-I`, etc.). |
| `ppflags.bxl` | Processes preprocessor flags into defines and include directories. |

### Build Integration

| File | Role |
|------|------|
| `msvs/vs_buck_build.props` | MSBuild property sheet that overrides Build/Clean/Rebuild to invoke `buck2`. |
| `msvs/absolutize_path.exe` | Windows utility for converting relative paths to absolute paths. |

## Key Design Patterns

- **Dynamic Output**: BXL scripts use `dynamic_output` because `resolved_attrs_lazy()` only resolves macros (like `$(location)`) at write time. Attributes are written to JSON first, then read back in a dynamic lambda.
- **Multi-Mode Support**: Multiple Buck mode files (e.g., debug/release) become separate VS configurations. Mode hashes translate content-based paths across configurations.
- **Dependency Aggregation**: Compiler/linker settings propagate bottom-up through the dependency tree. Exported settings are transitive; private settings apply only to the owning target.
- **Debug Inference**: Debug settings are inferred per target type: `cxx_binary` (direct), `alias` (follows actual), `command_alias` (handles oxx_runner, launcher, generic wrappers), `cxx_test` (handles BUCK_BASE_BINARY, AVATAR_LAUNCHER_TARGET).
- **Target Filtering**: `...` patterns are filtered by `recursive_target_types` (defaults: `cxx_binary`, `cxx_library`, `cxx_test`, `alias`, `command_alias`) and optional include/exclude patterns.

## Build Targets (BUCK)

- `vsgo` - Python binary entry point
- `vsgo_lib` - Python library
- `vsgo_bxl` - Filegroup of all `.bxl` files
- `absolutize_path_exe` - Exported Windows utility
- `vs_buck_build_props` - Exported MSBuild property sheet
