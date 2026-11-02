---
id: buckconfig
title: .buckconfig
---

The root of your [project](glossary.md#project) must contain a configuration
file named `.buckconfig`. Before executing, Buck2 reads this file to incorporate
any customizations it specifies.

## Performance impact of Buck2 configuration changes

Because configuration settings are sometimes included in the cache keys that
Buck2 uses in its caching system, changes to Buck's configuration can invalidate
previously-built artifacts in Buck's caches. If this occurs, Buck2 rebuilds
those artifacts, which can impact your build time.

These configuration changes can happen when modifying configuration files and
command line args. [See more](#precedence-of-buck2-configuration-specifications)

## The .buckconfig file uses the INI file format

The `.buckconfig` file uses the
[INI file format](http://en.wikipedia.org/wiki/INI_file). That is, it is divided
into _sections_ where each section contains a collection of key _names_ and key
_values_. The `.buckconfig` implementation supports some modifications to the
INI file format; these are discussed below.

### Other INI file parsers

As mentioned previously, we have extended the INI file parser that Buck2 uses to
parse configuration files. As a result, _INI file parsers provided by other
languages or libraries are often not able to parse Buck's configuration files
successfully_.

### Dot character not supported in section names

We do not support the use of the _dot_ character (`.`) in section names within
Buck2 configuration files. For example, the following is **not**
supported—_although Buck2 does not issue a warning or error_.

```ini
[foo.bar]
  baz=1
```

Note that sometimes you might need to define your own custom sections, such as
for platform flavors for C++ or Python. These scenarios are examples of when you
should be careful not to introduce the dot character in section names. This
constraint is because Buck2 uses the dot character to delimit section names and
key names in other contexts such as the `--config` command-line parameter.

## Character encoding

To ensure that any character can be encoded in a `.buckconfig` key value, you
can use escape sequences to encode characters that would otherwise be
problematic. The following escape sequences are supported.

| `\\`         | backslash                                           |
| ------------ | --------------------------------------------------- |
| `\"`         | double quote                                        |
| `\n`         | newline                                             |
| `\r`         | carriage return                                     |
| `\t`         | tab                                                 |
| `\x##`       | Unicode character with code point ## (in hex)       |
| `\u####`     | Unicode character with code point #### (in hex)     |
| `\U########` | Unicode character with code point ######## (in hex) |

## Key values as lists

Although the standard INI format supports only key values that represent a
single item, Buck2 supports key values that represent a list of items. The
syntax is to separate the items in the list using the space (`0x20`) character.
For example, a key value for the list of command-line flags to be passed to a
compiler could be represented as a list of the flags separated by spaces:

```ini
flags = -foo -bar -baz -qux
```

When a key value is parsed as a list instead of a single item, the separator
character is interpreted as a separator only when it occurs _outside of double
quotes_. For example, if `flags` is a key value interpreted as a list of items
separated by spaces, then

```ini
flags = -foo "-bar \u0429"
```

results in the two strings: `foo` and `-bar Щ`; the space character between
`-bar` and `\u0429` is not interpreted as a separator.

## Transclusion of values from one key to another

Values from other keys can be transcluded into the current key using the
following syntax inside the current key value.

```
$(config <section>.<field>)
```

For example, to use the `[go].vendor_path` in a custom setting:

```ini
[custom_section]custom_value = $(config go.vendor_path)
```

## Comments

In addition to the semicolon (`;`), you can use the pound sign (`#`), as a
comment character in `.buckconfig`.

## .buckconfig.local

The root of your [project](glossary.md#project) may contain a second
configuration file named `.buckconfig.local`. Its format is the same as that of
`.buckconfig`, but settings in `.buckconfig.local` override those in
`.buckconfig`. In practice, `.buckconfig` is a version-controlled file that
contains settings that are applicable to all team members, whereas
`.buckconfig.local` is excluded from version control to allow users to define
personal settings, such as personal aliases.

## Other initialization files

In addition to the `.buckconfig` and `.buckconfig.local` files in the project
root, Buck2 reads configuration settings from the following additional
locations, some of which are actually directories:

1. Directory `.buckconfig.d` located in the project root directory.
2. File `.buckconfig` and directory `.buckconfig.d` located in the current
   user's home directory which, on Unix-like systems, is available from the
   `HOME` environment variable or through the `~` symbol.
3. File `buckconfig` and directory `buckconfig.d` located in system directory
   `/etc/`.

Buck2 treats _any_ file—irrespective of name—in a
`.buckconfig.d`(`buckconfig.d`) directory (excluding files found in
subdirectories) as a Buck2 configuration file, provided that it adheres to
`.buckconfig` syntax. Note that a `.buckconfig.d` directory is distinct from the
similarly-named `.buckd` directory which is used by the
[Buck2 Daemon (`buckd`)](daemon.md) . For a description of how Buck2 resolves
collisions between settings in these configuration files, see the section
[**Precedence of Buck2 configuration specifications**](#precedence-of-buck2-configuration-specifications)
below.

## Command-line control of configuration

In addition to the above configuration files, Buck2 supports specifying
additional configuration files from the Buck2 command line using the
`--config-file` parameter. You can also specify configuration settings
_individually_ on the Buck2 command line using the `--config` (`-c`) parameter.
Furthermore, you can aggregate these settings into _flag files_ using the
`--flagfile` parameter. A flag file provides similar functionality to a
configuration file but uses a different syntax. Flag files are sometimes called
_mode files_ or _at_ (`@`) files.

## Precedence of Buck2 configuration specifications

The following list shows the order of precedence for how Buck2 interprets its
configuration specifications. Settings specified using a method closer to the
top of the list have higher precedence and will override those lower on the
list. For example, the `.buckconfig` file in the repo overrides a `.buckconfig`
file in the user's `HOME` directory.

1. Configuration specified on the command line using `--config` (`-c`),
   `--config-file` and `--flagfile`. Configuration specified later on the
   command line overrides configuration specified earlier.
1. `.buckconfig.local` in the repo.
1. `.buckconfig` in the repo.
1. Files in a `.buckconfig.d` folder of the repo.
1. `.buckconfig.local` in user's `HOME` directory.
1. Files in a `.buckconfig.d` folder in user's `HOME` directory.
1. The global file `/etc/buckconfig`
1. Files in the global directory `/etc/buckconfig.d`

Files in a `.buckconfig.d` (`buckconfig.d`) directory have precedence according
to the lexicographical order of their file names. Files _later_ in the
lexicographical order have precedence over files earlier in that order.

## Configuration files can include other files

Any of the configuration files that we've discussed so far can also include by
reference other files that contain configuration information. These included
files can contain complete `.buckconfig` sections or they can contain a group of
key name/value pairs that constitute part of a section. In this second use case,
you'll need to ensure that the _included_ file is referenced beneath the
appropriate section in the _including_ file. Because of this additional
complexity, we recommend that you include only files that contain complete
sections. **Note:** Inclusion of files is a Buck-specific extension to the INI
file parser that Buck2 uses. Therefore, if you use this feature, your Buck2
configuration files will probably not be parsable by other more-generic INI file
parsers. The syntax to include a file is

```
<file:*path-to-included-file*>
```

where _path-to-included-file_ is either a relative path from the including file
(recommended) or an absolute path from the root of the file system. You can also
specify that the file should be included only if it exists by prefixing with a
question mark (`?`).

```
<?file:*path-to-included-file*>
```

If you use this prefix, it is not an error condition if the file does not exist;
Buck2 just silently continues to process the rest of the configuration file. In
the following example, the `.buckconfig` file includes the file
`cxx-other-platform.include` which exists in the subdirectory
`cxx-other-platform`. The `.buckconfig` file will also include the file
`future-platform` from the directory `future-platform.include` if that file
exists.

```ini
#
# .buckconfig
#
[cxx]
  cxxppflags="-D MYMACRO=\"Buck\""

<file:cxx-other-platform/cxx-other-platform.include>

<?file:future-platform/future-platform.include>
#
# cxx-other-platform.include
#
[cxx#other_platform]
  cxxppflags="-D MYMACRO=\"Watchman\""
```

## Sections

Below is an incomplete list of supported buckconfigs.

## [alias]

This section contains definitions of [build target](build_target.md) aliases.

```ini
[alias]
  app = //apps/myapp:app
  apptest = //apps/myapp:test
```

These aliases can then be used from the command line:

```sh
$ buck2 build app
$ buck2 test apptest
```

## [cells]

Lists the cells that constitute the Buck2 project. Buck2 builds that are part of
this project—that is, which use this `.buckconfig`—can access the cells
specified in this section.

```ini
[cells]
    buck = .
    bazel_skylib = third-party/skylark/bazel-skylib
```

The string on the left-hand side of the equals sign is the _alias_ for the cell.
The string on the right-hand side of the equals sign is the normalized path to
the cell relative to the project root, or `.` for the project root itself. It is
not necessary to include the current cell in this section, but we consider it a
best practice to do so:

```ini
buck = .
```

You can view the contents of this section using the `buck2 audit cell` command.

`[repositories]` is additionally supported as a deprecated alternative name for
this section.

## [project]

### ignore

A comma-separated list of patterns, relative to the cell, for paths that Buck2
treats as absent: they are left out of directory listings and globs, changes to
them are not noticed, and reading one that is named explicitly is refused. A
pattern without glob characters names a directory and covers everything under
it. One with glob characters is matched like a gitignore pattern, except that
`*` does not cross `/`. `buck-out` is always ignored in the root cell.

A pattern starting with `!` un-ignores the paths it matches, whatever other
patterns say about them, and only those paths: under a directory that stays
ignored, everything stays ignored.

```ini
[project]
  ignore = \
    **/*.xcodeproj, \
    **/*.xcodeproj/**, \
    !apps/*/*.xcodeproj, \
    !apps/*/*.xcodeproj/project.pbxproj
```

Here every Xcode project is ignored, except that the project files of those
directly under `apps` are not. Those directories are un-ignored too, since a
file under an ignored directory could not be. Everything else in them, such as
the state Xcode writes while a project is open, stays ignored.
