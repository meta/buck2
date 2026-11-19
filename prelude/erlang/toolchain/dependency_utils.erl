%%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%%
%%% This source code is licensed under both the MIT license found in the
%%% LICENSE-MIT file in the root directory of this source tree and the Apache
%%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%%% of this source tree.

%% @format
-module(dependency_utils).
-compile(warn_missing_spec_all).

-export([chars_to_binary/1]).

-spec chars_to_binary(StringLike :: iodata()) -> binary().
chars_to_binary(StringLike) ->
    case unicode:characters_to_binary(StringLike) of
        Bin when is_binary(Bin) -> Bin
    end.
