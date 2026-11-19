%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(listing_interfacer).

-compile(warn_missing_spec_all).

-export([produce_json_file/2]).

-include_lib("common/include/tpx_records.hrl").

-import(common_util, [unicode_characters_to_binary/1]).

-spec produce_json_file(OutputDir, TestCase) -> ok when
    OutputDir :: file:filename_all(),
    TestCase :: #test_spec_test_case{}.
produce_json_file(OutputDir, TestCase) ->
    Filename = filename:join(OutputDir, ~"result"),
    {ok, Fd} = file:open(Filename, [write, raw, binary]),
    try
        lists:foreach(
            fun(TestInfo) ->
                TestInfoJson = test_info_to_json_line(TestInfo),
                ok = file:write(Fd, [TestInfoJson, ~"\n"])
            end,
            TestCase#test_spec_test_case.testcases
        )
    after
        file:close(Fd)
    end.

-spec test_info_to_json_line(TestInfo) -> iodata() when
    TestInfo :: #test_spec_test_info{}.
test_info_to_json_line(TestInfo) ->
    {M, F, A} = TestInfo#test_spec_test_info.breakpoint,
    BreakpointFun = unicode_characters_to_binary(io_lib:format(~"~s:~s/~p", [M, F, A])),

    % NB. json:encode() guarantees the output is in one line
    json:encode(#{
        testcase => TestInfo#test_spec_test_info.name,
        filter => TestInfo#test_spec_test_info.filter,
        breakpoint => #{type => function, functionName => BreakpointFun}
    }).
