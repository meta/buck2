---
id: language_support
title: Language / Ecosystem Support
---

# Language / Ecosystem Support

This page provides an overview of the programming languages supported by Buck2.

| Language / Ecosystem | Prelude Available | Usability     | Documented | Maintainers                   |
| -------------------- | ----------------- | ------------- | ---------- | ----------------------------- |
| C/C++ (not windows)  | ✅                | Complex Setup | ❌         |                               |
| C/C++ (windows)      | ✅                | Complex Setup | ❌         |                               |
| C#                   | ✅                | Unavailable   | ❌         |                               |
| Erlang               | ✅                | Easy Setup    | ❌         | GitHub: michalmuskala         |
| Go                   | ✅                | Easy Setup    | ❌         | GitHub: podtserkovskiy        |
| Haskell              | ✅                | Easy Setup    | ❌         |                               |
| Java                 | ✅                | Complex Setup | ❌         |                               |
| Java (Mobile)        | ✅                | Complex Setup | ❌         | GitHub: NavidQar & IanChilds  |
| Kotlin               | ✅                | Complex Setup | ❌         |                               |
| Kotlin (Mobile)      | ✅                | Complex Setup | ❌         | GitHub: NavidQar & siaojiecai |
| Objective-C          | ✅                | Unavailable   | ❌         |                               |
| OCaml                | ✅                | Easy Setup    | ❌         |                               |
| Python               | ✅                | Easy Setup    | ❌         | GitHub: zsol                  |
| Rust                 | ✅                | Easy Setup    | ❌         | GitHub: jakobdegen            |
| Swift                | ✅                | Unavailable   | ❌         |                               |

## Understanding the Table

- **Prelude Available**: Indicates whether Buck2's prelude includes built-in
  rules for this language.
- **Usability**: Indicates whether this language is possible or degree of setup
  required.
  - **Easy Setup**: Basic installation required, usually searching the path for
    tools
  - **Complex Setup**: Requires additional setup beyond simply installation
  - **Unavailable**: Rules are using tools that are not available
- **Documented**: Indicates the level of documentation available for using this
  language with Buck2.
- **Maintainers**: Teams or individuals responsible for maintaining support for
  this language.

## Adding Support for New Languages

Buck2 is designed to be extensible, allowing you to add support for additional
programming languages. To add support for a new language, you typically need to:

1. Define appropriate build rules in a `.bzl` file
2. Create toolchain definitions for the language
3. Write documentation for how to use the language with Buck2

For more information on creating custom rules and toolchains, see:

- [Writing Rules](../rule_authors/writing_rules.md)
- [Writing Toolchains](../rule_authors/writing_toolchains.md)
