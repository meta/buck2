%% Copyright (c) Meta Platforms, Inc. and affiliates.
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.
%%% % @format
-module(test_cli_e2e_SUITE).
-typing([eqwalizer]).

-export([suite/0, all/0, init_per_suite/1, end_per_suite/1]).

-export([
    test_list/1,
    test_try_make_path_relative/1
]).

% elp:ignore WA003 (better_assertions) - Open Source
-include_lib("stdlib/include/assert.hrl").
-include_lib("common_test/include/ct.hrl").
-include_lib("common/include/buck_ct_records.hrl").

suite() ->
    [{appatic, #{enable_autoclean => true}}].

all() ->
    [test_list, test_try_make_path_relative].

init_per_suite(Config) ->
    PrivDir = proplists:get_value(priv_dir, Config),
    TestInfoFile = filename:join(PrivDir, <<"test_info">>),

    {ok, [ErlCmd]} = init:get_argument(progname),

    TestSuitePath = [_ | _] = code:which(test_list_SUITE),
    test_info:write_to_file(TestInfoFile, #test_info{
        common_app_env = #{},
        raw_target = ~"raw_target",
        dependencies = [],
        test_suite = list_to_binary(TestSuitePath),
        config_files = [],
        providers = [],
        ct_opts = [],
        erl_cmd = [list_to_binary(Arg) || Arg <:- ErlCmd],
        extra_flags = [],
        artifact_annotation_mfa = fun artifact_annotations:default_annotation/1,
        trampolines = []
    }),

    application:set_env(test_cli_lib, test_info_file, TestInfoFile),

    Config.

end_per_suite(_Config) ->
    ok.

test_list(_Config) ->
    Expected =
        "test_cli_e2e_SUITE:\n"
        "test_list_SUITE:\n"
        "\t2 - test_list_SUITE - .test_pass\n"
        "\t3 - test_list_SUITE - default.test_fail\n"
        "\t4 - test_list_SUITE - .test_extended_ascii_£\n"
        "\t5 - test_list_SUITE - .test_unicode_🫠\n",
    ?assertEqual({ok, Expected}, test:list_impl("test_list_SUITE")),

    EmojiExpected =
        "test_cli_e2e_SUITE:\n"
        "test_list_SUITE:\n"
        "\t5 - test_list_SUITE - .test_unicode_🫠\n",
    ?assertEqual({ok, EmojiExpected}, test:list_impl("_🫠")),

    ?assertMatch({error, {invalid_regex, _}}, test:list_impl("^[a")),

    EmptyExpected =
        "test_cli_e2e_SUITE:\n"
        "test_list_SUITE:\n",
    ?assertEqual({ok, EmptyExpected}, test:list_impl("does_not_exist_SUITE")).

%% Regression: lists:split/2 crashed with badarg when Path had fewer
%% components than CWD (e.g. sandcastle deep buck-out CWD vs short
%% dotslash cache ErlCmd path).
test_try_make_path_relative(_Config) ->
    ?assertEqual("relative/path", test_info:try_make_path_relative("relative/path")),

    {ok, CWD} = file:get_cwd(),
    ChildPath = filename:join(CWD, "some/child"),
    ?assertEqual("some/child", test_info:try_make_path_relative(ChildPath)),

    %% The crash case: absolute path shorter than CWD
    ShortAbsPath = "/short/path",
    ?assertEqual(ShortAbsPath, test_info:try_make_path_relative(ShortAbsPath)),

    DisjointPath = "/completely/different/tree/structure/file",
    ?assertEqual(DisjointPath, test_info:try_make_path_relative(DisjointPath)).
