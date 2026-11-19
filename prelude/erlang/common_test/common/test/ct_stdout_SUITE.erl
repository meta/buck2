%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
-module(ct_stdout_SUITE).
-typing([eqwalizer]).

-export([all/0]).

-export([
    test_collect_method_stdout_sanitizes_invalid_utf8_in_truncated_prefix/1,
    test_collect_method_stdout_sanitizes_invalid_utf8_in_truncated_middle/1,
    test_collect_method_stdout_sanitizes_invalid_utf8_without_truncation/1
]).

% elp:ignore WA003 (better_assertions) - Open Source
-include_lib("stdlib/include/assert.hrl").

-define(UNICODE_REPLACEMENT_CHAR_UTF8, <<239, 191, 189>>).

all() ->
    [
        test_collect_method_stdout_sanitizes_invalid_utf8_in_truncated_prefix,
        test_collect_method_stdout_sanitizes_invalid_utf8_in_truncated_middle,
        test_collect_method_stdout_sanitizes_invalid_utf8_without_truncation
    ].

test_collect_method_stdout_sanitizes_invalid_utf8_in_truncated_prefix(Config) ->
    Slice = <<16#9F, 16#98, 16#80, $A>>,
    Content = <<"pad", Slice/binary>>,
    Max = byte_size(Slice),
    ?assertEqual(
        {truncated, <<?UNICODE_REPLACEMENT_CHAR_UTF8/binary, "A">>},
        collect_stdout(Content, Max, Config)
    ).

test_collect_method_stdout_sanitizes_invalid_utf8_in_truncated_middle(Config) ->
    Slice = <<"prefix", 16#DD, 16#F5, $G, "tail">>,
    Content = <<"pad", Slice/binary>>,
    Max = byte_size(Slice),
    ?assertEqual(
        {truncated, <<"prefix", ?UNICODE_REPLACEMENT_CHAR_UTF8/binary, "Gtail">>},
        collect_stdout(Content, Max, Config)
    ).

test_collect_method_stdout_sanitizes_invalid_utf8_without_truncation(Config) ->
    Content = <<"prefix", 16#DD, 16#F5, $G, "tail">>,
    ?assertEqual(
        <<"prefix", ?UNICODE_REPLACEMENT_CHAR_UTF8/binary, "Gtail">>,
        collect_stdout(Content, byte_size(Content), Config)
    ).

-spec collect_stdout(binary(), non_neg_integer(), proplists:proplist()) ->
    binary() | {truncated, binary()}.
collect_stdout(Content, Max, Config) ->
    PrivDir = proplists:get_value(priv_dir, Config),
    File = filename:join(
        PrivDir,
        io_lib:format("ct_stdout_~p.log", [erlang:unique_integer([positive])])
    ),
    ok = file:write_file(File, Content, [raw, binary]),
    StartMarker = <<"start">>,
    EndMarker = <<"end">>,
    TreeResults = #{
        name => testcase,
        type => leaf,
        init_method => none,
        end_method => none,
        main_method => #{
            name => testcase,
            start_progress_marker => StartMarker,
            end_progress_marker => EndMarker,
            outcome => passed,
            details => ~""
        }
    },
    Offsets = #{StartMarker => 0, EndMarker => byte_size(Content)},
    {ok, CollectedStdOut} = ct_stdout:collect_method_stdout(File, Offsets, TreeResults, Max),
    maps:get(StartMarker, CollectedStdOut).
