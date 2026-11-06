---
id: subdir_glob
title: subdir_glob()
---

# subdir_glob()

The `subdir_glob()` function is a utility for creating dictionaries that map
relative paths to source files, making it particularly useful for defining
header maps for C/C++ libraries and resource mappings for Android projects. For
more information, see [`glob()`](../api/build/index.md#glob)

## Overview

`subdir_glob()` takes a list of (subdirectory, glob-pattern) tuples and returns
a dictionary where:

- **Keys** are paths relative to the specified subdirectories
- **Values** are the full paths to the matched files

This is especially useful when you need to maintain a specific directory
structure in your build outputs, such as header include paths that should be
relative to a base directory.

## Syntax

```python
subdir_glob(
    glob_specs,
    exclude = [],
    prefix = "",
)
```

## Arguments

- **`glob_specs`** (required): A list of tuples in the form
  `(relative-sub-directory, glob-pattern)`. Each tuple specifies:
  - A subdirectory path relative to the build file
  - A glob pattern to match files within that subdirectory
- **`exclude`** (optional, defaults to `[]`): A list of patterns identifying
  files that should be excluded from the results.

- **`prefix`** (optional, defaults to `""`): A string to prepend to each key in
  the resulting dictionary.

## Return Value

Returns a dictionary mapping relative paths (with optional prefix) to full file
paths.

## How It Works

For each tuple `(subdir, pattern)` in `glob_specs`:

1. Matches files using `glob([subdir/pattern])`
2. Creates dictionary entries where:
   - Key = file path relative to `subdir` (optionally prefixed)
   - Value = full path to the file

If multiple tuples are provided, their results are merged into a single
dictionary. If there are conflicting keys with different values, the function
will fail with an error.

## Examples

### Basic C++ Header Mapping

```python
cxx_library(
    name = "mylib",
    exported_headers = subdir_glob([
        ("include", "**/*.h"),
    ]),
)
```

For this directory structure:

```
include/
  math/
    vector.h
    matrix.h
  utils/
    helpers.h
```

This creates a mapping:

```python
{
    "math/vector.h": "include/math/vector.h",
    "math/matrix.h": "include/math/matrix.h",
    "utils/helpers.h": "include/utils/helpers.h",
}
```

Consumers can include headers as: `#include "math/vector.h"`

### Multiple Subdirectories with Prefix

```python
exported_headers = subdir_glob([
    ("lib/source", "video/**/*.h"),
    ("lib/source", "audio/**/*.h"),
  ],
  exclude = [
    "lib/source/video/codecs/*.h",
  ],
  prefix = "MediaLib/")
```

For this directory structure:

```
lib/
  source/
    video/
      converter/
        converter.h
      player/
        player.h
      codecs/
        codec1.h   (excluded)
        codec2.h   (excluded)
    audio/
      codecs/
        codec1.h
        codec2.h
      player/
        player.h
    internal/
      otherheader.h
```

This creates a mapping:

```python
{
    "MediaLib/video/converter/converter.h": "lib/source/video/converter/converter.h",
    "MediaLib/video/player/player.h": "lib/source/video/player/player.h",
    "MediaLib/audio/codecs/codec1.h": "lib/source/audio/codecs/codec1.h",
    "MediaLib/audio/codecs/codec2.h": "lib/source/audio/codecs/codec2.h",
    "MediaLib/audio/player/player.h": "lib/source/audio/player/player.h",
}
```

Consumers can include headers as:
`#include "MediaLib/video/converter/converter.h"`

Note that:

- Files in `lib/source/video/codecs/` are excluded
- Files in `lib/source/internal/` are not matched by any pattern

## Common Patterns

### Multiple File Types

```python
exported_headers = subdir_glob([
    ("include", "**/*.h"),
    ("include", "**/*.hpp"),
])
```

### Excluding Test Files

```python
headers = subdir_glob([
    ("src", "**/*.h"),
  ],
  exclude = [
    "src/**/*_test.h",
    "src/test/**",
  ])
```
