This page contains notes and tips to assist you in understanding the different
tools used when migrating Buck2 to Windows.

## CMD, Powershell, Bash Command Comparison

| Bash               | Powershell         | CMD              | What does it do                  |
| ------------------ | ------------------ | ---------------- | -------------------------------- |
| cd                 | cd                 | cd               | Change the current directory     |
| mkdir              | mkdir              | mkdir / md       | Create a directory               |
| ls                 | ls                 | dir              | List contents of a directory     |
| export var="value" | $env:var="value"   | set var=value    | To set environment variables     |
| $ENV_VAR           | $env:ENV_VAR       | %ENV_VAR%        | Read environment variable        |
| echo "Hello world" | echo "Hello world" | echo Hello world | To print something on the screen |
| rm                 | rm                 | del              | Delete a file                    |
| rm -rf             | rmdir              | rmdir            | Delete a directory               |
| cat                | cat                | type             | Print file content to console    |

## Symlinks

In Windows, there are two types of symlinks: file and directory.

You can find out which type of symlink is being created using:
`dir /AL /S <path>`.

The command lists all of the symbolic links in the `<path>` directory:

- `^<SYMLINKD^>` is a Directory SymLink
- `^<SYMLINK^>` is a File SymLink

## Target names

Escaping the '=' symbol on Windows is quite complicated: make sure none of the
targets being built contain this symbol as it could cause build breakages.
