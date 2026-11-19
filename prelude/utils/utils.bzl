# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# General utilities shared between multiple rules.

load("@prelude//utils:expect.bzl", "expect")

def as_output(artifact: Artifact | OutputArtifact) -> OutputArtifact:
    """
    Convert an Artifact or OutputArtifact to an OutputArtifact.
    Useful when a function needs to accept either type.
    """
    if isinstance(artifact, OutputArtifact):
        return artifact
    else:
        return artifact.as_output()

def value_or(x: [None, typing.Any], default: typing.Any) -> typing.Any:
    return default if x == None else x

def values_or(*xs: typing.Any | None) -> typing.Any | None:
    for x in xs:
        if x != None:
            return x
    return None

# Flatten a list of lists into a list
def flatten(xss: list[list[typing.Any]]) -> list[typing.Any]:
    return [x for xs in xss for x in xs]

# Flatten a list of dicts into a dict
def flatten_dict(xss: list[dict[typing.Any, typing.Any]]) -> dict[typing.Any, typing.Any]:
    return {k: v for xs in xss for k, v in xs.items()}

def from_named_set(srcs: [dict[str, Artifact | Dependency], list[Artifact | Dependency]]) -> dict[str, Artifact | Dependency]:
    """
    Normalize parameters of optionally named sources to a dictionary mapping
    names to sources, deriving the name from the short path when it's not
    explicitly provided.
    """

    if type(srcs) == type([]):
        srcs_dict = {}
        artifacts = {}
        for src in srcs:
            if isinstance(src, Artifact):
                artifact = src
            else:
                # If the src is a `dependency`, use the short path of the
                # default output.
                expect(
                    len(src[DefaultInfo].default_outputs) == 1,
                    "expected exactly one default output from {} ({})".format(src, src[DefaultInfo].default_outputs),
                )
                [artifact] = src[DefaultInfo].default_outputs
            name = artifact.short_path
            expect(
                artifacts.setdefault(name, artifact) == artifact,
                "Sources {} and {} both have the name `{}`",
                artifacts[name],
                artifact,
                name,
            )
            srcs_dict[name] = src
        return srcs_dict
    else:
        return srcs

def map_idx(key: typing.Any, vals: list[typing.Any]) -> list[typing.Any]:
    return [x[key] for x in vals]

def filter_and_map_idx(key: typing.Any, vals: list[typing.Any]) -> list[typing.Any]:
    return [x[key] for x in vals if key in x]

def idx(x: [typing.Any, None], key: typing.Any) -> [typing.Any, None]:
    return x[key] if x != None else None

def dedupe_by_value(vals: list[typing.Any]) -> list[typing.Any]:
    return list(set(vals))

def map_val(func: typing.Callable[[typing.Any], typing.Any], val: [typing.Any, None]) -> [typing.Any, None]:
    """
    If `val` if `None`, return `None`, else apply `func` to `val` and return the
    result.
    """

    if val == None:
        return None

    return func(val)
