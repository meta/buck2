# VSGO: Visual Studio project generation for buck targets

VSGO is a project generator that generates Visual Studio project and solution
files given buck target labels or patterns, with full functional IntelliSense
and build/run/debug setup.

The majority of heavy lifting is implemented in
[BXL](https://buck2.build/docs/developers/bxl/), Buck Extension Language, which
offers superb performance and handles incremental changes extremely well.

![demo](assets/demo.png)

Windows projects target the Visual Studio 2022 IDE. The selected Buck modes
still determine the compiler, architecture, SDK and CRT; choosing an IDE version
does not migrate a toolchain. `gen_mode_configs.bxl` accepts only `2022` for
`--vs_version_year`. The ARVR wrapper accepts `--vs2022` or its default, including
for optional natvis routing.

## Quick Start

Use project "buck2/examples/vscode" as an example,

```
mkdir modes && echo "" > modes/debug  # One-time setup. Skip if there is one already exists.
python prelude/ide_integrations/visual_studio/vsgo.py root//:main --mode_files modes/debug
```

This will generate project and solution files for the specific target and output
file paths at the end. From there, you can continue with Visual Studio to
browse, edit, and run/debug your buck target.

To list all options and their help docs, run

```
python prelude/ide_integrations/visual_studio/vsgo.py --help
```

## How It Works

![how-it-works.png](assets/how-it-works.png)
