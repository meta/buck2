%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(provider_listing_SUITE).

-export([all/0, groups/0, test_pass/1]).

all() ->
    [test_pass, {group, outer}].

groups() ->
    [{outer, [test_pass, {group, 'inner_🫠'}]}, {'inner_🫠', [test_pass]}].

test_pass(_Config) ->
    ok.
