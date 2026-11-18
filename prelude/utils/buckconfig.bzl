# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Provides macros for working with .buckconfig."""

load(":expect.bzl", "expect")
load(":lazy.bzl", "lazy")

def _decode_raw_word(val: str, start: int, delimiter: str | None = None) -> (int, str):
    """
    Read characters up to the given delimiter in a string supporting and
    stripping quoting (i.e. `"`) and escape characters (i.e. `\\`).

    Args:
      val: Input string to evaluate.
      start: Where to start in the input string.
      delimiter: An optional character to terminate at.  If omitted will
                 continue to the end of the string.
    """

    quotes = ['"', "'"]
    word = ""
    current_quote_char = None
    escaped = False
    idx = -1

    for idx in range(start, len(val)):
        c = val[idx]

        if current_quote_char == None and c == delimiter:
            break

        if current_quote_char == None and c in quotes:  # quote start
            current_quote_char = c
        elif c == current_quote_char and not escaped:  # quote end
            current_quote_char = None
        elif c == "\\" and not escaped:  # handle escape char
            expect(
                current_quote_char != None,
                "escape char outside of quotes at char %d in: %s" % (idx + 1, val),
            )
            escaped = True
        else:
            word += c
            escaped = False

    expect(current_quote_char == None, "quote not closed in: %s" % val)

    return idx, word

def _next_word(val: str, start: int, delimiter: str | None) -> int:
    """
    Advance past delimiter characters.
    """

    for idx in range(start, len(val)):
        c = val[idx]
        if c != delimiter:
            return idx

    return -1

# Below are some utilities to log and track `read_config` calls
# You can enable this with `-c buckconfig.log=<a single key>`, ex. `-c buckconfig.log=build.use_limited_hybrid`.
# To print in json, use `-c buckconfig.log_json=build.use_limited_hybrid`.
# This will print a stacktrace on stderr for every `read_config` call of `build.use_limited_hybrid`.
# Would recommend piping the stderr to file because otherwise printing to stderr can be slow and hard to read.
# You can also print a record for all keys with `-c buckconfig.log_all_in_json=true`. This prints all `read_config` calls
# without stacktraces, where each entry is a JSON that includes the cell, section, and key.
# NOTE: even without stacktraces, log all mode is extremely slow and memory hungry, often 20-40x slower and more
# memory hungry than the equivalent run without, so only use it on small graphs or when absolutely necessary.

def _buck_config_log_keys(json: bool) -> set[(str, str)]:
    log_section_and_key = read_root_config("buckconfig", "log_json" if json else "log")
    if not log_section_and_key:
        return set()

    # Unfortunately, due to buckconfigs allowing `.`, it's possible to have multiple
    # ambiguous section/key for a single buckconfig, so check for that here.
    result = set()
    splits = log_section_and_key.split(".")
    for i in range(1, len(splits)):
        section = ".".join(splits[:i])
        key = ".".join(splits[i:])
        result.add((section, key))
    return result

_BUCKCONFIG_LOG_KEYS = _buck_config_log_keys(json = False)
_BUCKCONFIG_LOG_JSON_KEYS = _buck_config_log_keys(json = True)
_BUCKCONFIG_LOG_ALL = read_root_config("buckconfig", "log_all_in_json") in ("True", "true")
LOG_BUCKCONFIGS = bool(_BUCKCONFIG_LOG_KEYS or _BUCKCONFIG_LOG_JSON_KEYS or _BUCKCONFIG_LOG_ALL)

# optional fields
_BUCKCONFIG_LOG_CALLSTACK = read_root_config("buckconfig", "log_callstack") in ("True", "true")
_BUCKCONFIG_LOG_VALUE = read_root_config("buckconfig", "log_value") in ("True", "true")

def _log_read_config(read_func, section: str, key: str, default = None):
    value = read_func(section, key, default)

    maybe_value_dict = {"value": value} if _BUCKCONFIG_LOG_VALUE else {}
    maybe_callstack_dict = {"call_stack": call_stack()} if _BUCKCONFIG_LOG_CALLSTACK else {}
    if _BUCKCONFIG_LOG_ALL:
        output = {
            "starlark_log_all_buckconfigs": {
                "cell": get_cell_name(),
                "key": key,
                "section": section,
            }
            | maybe_value_dict
            | maybe_callstack_dict,
        }

        # This only prints if buckconfig is set
        # buildifier: disable=print
        print(json.encode(output))

    if _BUCKCONFIG_LOG_JSON_KEYS:
        if (section, key) in _BUCKCONFIG_LOG_JSON_KEYS:
            output = {
                "starlark_log_buckconfig": {
                    "call_stack": call_stack(),
                    "cell": get_cell_name(),
                }
                | maybe_value_dict,
            }

            # This only prints if buckconfig is set
            # buildifier: disable=print
            print(json.encode(output))

    if _BUCKCONFIG_LOG_KEYS:
        if (section, key) in _BUCKCONFIG_LOG_KEYS:
            # This only prints if buckconfig is set
            # Need to do everything in one print statement because otherwise lines from parallel print
            # invocations at load time will get interlaced
            # buildifier: disable=print
            print("========starlark_log_buckconfig========\n{}\n".format(call_stack()))

    return value

read_config_with_logging = partial(_log_read_config, read_config)
read_root_config_with_logging = partial(_log_read_config, read_root_config)

_read_config = read_config_with_logging if LOG_BUCKCONFIGS else read_config
_read_root_config = read_root_config_with_logging if LOG_BUCKCONFIGS else read_root_config

def read(section: str, field: str, default: str | Select | None = None, root_cell: bool = False, logging: bool = True) -> str | Select | None:
    """Read a `string` from `.buckconfig`."""
    if logging:
        read_config_func = _read_root_config if root_cell else _read_config
    else:
        read_config_func = read_root_config if root_cell else read_config
    return read_config_func(section, field, default)

# Alias for `read` that's explicit about the type being returned.
read_string = read

def read_choice(
    section: str, field: str, choices: typing.Iterable, default: str | Select | None = None, required: bool = True, root_cell: bool = False
) -> str | Select | None:
    """Read a string from `.buckconfig` that must be one `choices`."""

    val = read(section, field, root_cell = root_cell)
    if val != None:
        if val in choices:
            return val
        else:
            fail(
                "`{}:{}`: must be one of ({}), but was {}".format(section, field, ", ".join(choices), repr(val)),
            )
    elif default != None:
        return default
    elif not required:
        return None
    else:
        fail("`{}:{}`: no value set".format(section, field))

def read_bool(
    section: str, field: str, default: bool | Select | None = None, required: bool = True, root_cell: bool = False, logging: bool = True
) -> bool | Select | None:
    """Read a `boolean` from `.buckconfig`."""

    # Treat the empty string as "unset".  This allows the user to "override" a
    # previous setting by "clearing" it out.
    val = read(section, field, root_cell = root_cell, logging = logging)
    if val != None and val != "":
        # Fast-path string check
        if val == "True" or val == "true":
            return True
        elif val == "False" or val == "false":
            return False

        # Else fall back to lower casing
        if val.lower() == "true":
            return True
        elif val.lower() == "false":
            return False
        else:
            fail(
                "`{}:{}`: cannot coerce {!r} to bool".format(section, field, val),
            )
    elif default != None:
        return default
    elif not required:
        return None
    else:
        fail("`{}:{}`: no value set".format(section, field))

def read_int(section: str, field: str, default: int | Select | None = None, required: bool = True, root_cell: bool = False) -> int | Select | None:
    """Read an `int` from `.buckconfig`."""

    val = read(section, field, root_cell = root_cell)
    if val != None:
        if val.isdigit():
            return int(val)
        else:
            fail(
                "`{}:{}`: cannot coerce {!r} to int".format(section, field, val),
            )
    elif default != None:
        return default
    elif not required:
        return None
    else:
        fail("`{}:{}`: no value set".format(section, field))

def read_list(
    section: str, field: str, delimiter: str | None = ",", default: typing.Iterable | Select | None = None, required: bool = True, root_cell = False
) -> typing.Iterable | Select | None:
    """Read a `list` from `.buckconfig`."""
    val = read(section, field, root_cell = root_cell)
    if val != None:
        quotes = ["\\", '"', "'"]
        if lazy.is_any(lambda x: x in val, quotes):
            words = []
            idx = 0
            for _ in range(len(val)):
                idx = _next_word(val, idx, delimiter)
                if idx == -1:
                    break
                idx, word = _decode_raw_word(val, idx, delimiter)
                words.append(word.strip())
                if idx == -1 or idx >= len(val) - 1:
                    break
            return words
        else:
            return [v.strip() for v in val.split(delimiter) if v]
    elif default != None:
        return default
    elif not required:
        return None
    else:
        fail("`{}:{}`: no value set".format(section, field))

def resolve_alias(alias: str) -> str:
    """Resolves an alias into a target (recursively). `fail`s if the alias does
    not exist.

    Args:
        alias (str): The alias or target to resolve.

    Returns:
        The target pointed to by the alias (or the input if the caller lied
        to us and `alias` is a target)
    """
    if "//" in alias:
        return alias

    # Starlark doesn't have while loops or allow recursion, so if we want to
    # resolve aliases that we find we need to iterate somehow
    for _ in range(1000):
        # TODO: set root_cell to true when all aliases come from root cell?
        target = read("alias", alias, root_cell = False)
        expect(target != None, "Alias {} does not exist".format(alias))
        if "//" in target:
            return target
        else:
            alias = target
    fail("This should never happen - either the alias exists or it doesn't")
