%% Copyright (c) Meta Platforms, Inc. and affiliates.
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.
%%% % @format
-module(test_list_SUITE).

-export([all/0, groups/0]).

-export([
    test_pass/1,
    test_fail/1,
    'test_extended_ascii_£'/1,
    'test_unicode_🫠'/1
]).

-include_lib("stdlib/include/assert.hrl").

all() ->
    [test_pass, {group, default}, 'test_extended_ascii_£', 'test_unicode_🫠'].

groups() ->
    [{default, [], [test_fail]}].

test_pass(_Config) ->
    ?assert(true).

test_fail(_Config) ->
    ?assert(false).

'test_extended_ascii_£'(_Config) ->
    ?assert('£').

'test_unicode_🫠'(_Config) ->
    ?assert('🫠').
