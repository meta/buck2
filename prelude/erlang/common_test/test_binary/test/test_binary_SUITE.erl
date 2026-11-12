%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(test_binary_SUITE).

-export([all/0]).
-export([
    test_provider_callbacks/1,
    test_post_listing_optional/1,
    test_post_listing_errors/1,
    test_post_listing_invalid_return/1,
    test_post_listing_invalid_names/1,
    test_listing_providers/1,
    test_listing_preserves_order/1,
    test_listing_filters_all/1
]).

-include_lib("stdlib/include/assert.hrl").
-include_lib("common/include/buck_ct_records.hrl").

all() ->
    [
        test_provider_callbacks,
        test_post_listing_optional,
        test_post_listing_errors,
        test_post_listing_invalid_return,
        test_post_listing_invalid_names,
        test_listing_providers,
        test_listing_preserves_order,
        test_listing_filters_all
    ].

test_provider_callbacks(_Config) ->
    InitState = #init_provider_state{
        output_dir = "unused", suite = provider_listing_SUITE, raw_target = ~"test_binary_SUITE"
    },
    ?assertEqual({list_test, #{}}, buck_ct_provider:do_init({list_test, []}, InitState)),
    Provider = {list_test, #{state => unchanged}},
    ?assertEqual(Provider, buck_ct_provider:do_pre_listing(Provider)),
    ?assertEqual(Provider, buck_ct_provider:do_pre_running(Provider)),
    ?assertEqual(Provider, buck_ct_provider:do_post_running(Provider)),
    ?assertEqual(Provider, buck_ct_provider:do_terminate(Provider)),
    ?assertEqual({listing_provider, updated}, buck_ct_provider:do_pre_listing({listing_provider, update})),
    ?assertEqual({listing_provider, unexpected}, buck_ct_provider:do_pre_listing({listing_provider, unexpected})).

test_post_listing_optional(_Config) ->
    TestNames = test_names(),
    Provider = {list_test, #{state => unchanged}},
    ?assertEqual({Provider, TestNames}, buck_ct_provider:do_post_listing(Provider, TestNames)),
    ?assertEqual({Provider, []}, buck_ct_provider:do_post_listing(Provider, [])).

test_post_listing_errors(_Config) ->
    ?assertThrow(
        {error_in_provider, {listing_provider, test_error}},
        buck_ct_provider:do_post_listing({listing_provider, error}, test_names())
    ),
    ?assertThrow(
        {crash_in_provider, {error, test_crash, _}},
        buck_ct_provider:do_post_listing({listing_provider, crash}, test_names())
    ).

test_post_listing_invalid_return(_Config) ->
    ?assertError(
        {invalid_post_listing_return, listing_provider, unexpected, ~"expected {ok, [binary()], State}"},
        buck_ct_provider:do_post_listing({listing_provider, unexpected}, test_names())
    ),
    ?assertError(
        {invalid_post_listing_return, listing_provider, {ok, invalid_list, invalid_list},
            ~"expected {ok, [binary()], State}"},
        buck_ct_provider:do_post_listing({listing_provider, invalid_list}, test_names())
    ).

test_post_listing_invalid_names(_Config) ->
    ?assertError(
        {invalid_test_name_from_provider, listing_provider, invalid_test_name, ~"expected a binary"},
        buck_ct_provider:do_post_listing({listing_provider, invalid_test_name}, test_names())
    ),
    ?assertError(
        {unknown_test_name_from_provider, listing_provider, ~".missing"},
        buck_ct_provider:do_post_listing({listing_provider, unknown_test_name}, test_names())
    ).

test_listing_providers(Config) ->
    [TopTest, GroupTest, NestedTest] = TestNames = test_names(),
    OutputDir = proplists:get_value(priv_dir, Config),
    Providers = [
        {listing_provider, provider_args(TestNames, [TopTest, NestedTest], "first")},
        {list_test, []},
        {listing_provider, provider_args([TopTest, NestedTest], [NestedTest], "second")}
    ],
    run_test_binary(Providers, OutputDir, ["list", "--output-dir", OutputDir]),
    {ok, Result} = file:read_file(filename:join(OutputDir, "result")),
    ?assertEqual(
        #{
            <<"testcase">> => NestedTest,
            <<"filter">> => NestedTest,
            <<"breakpoint">> => #{
                <<"type">> => <<"function">>, <<"functionName">> => <<"provider_listing_SUITE:test_pass/1">>
            }
        },
        json:decode(Result)
    ),
    ?assertNotEqual(GroupTest, NestedTest),
    ?assertEqual({ok, term_to_binary([TopTest, NestedTest])}, file:read_file(filename:join(OutputDir, "first"))),
    ?assertEqual({ok, term_to_binary([NestedTest])}, file:read_file(filename:join(OutputDir, "second"))).

test_listing_preserves_order(Config) ->
    TestNames = test_names(),
    OutputDir = proplists:get_value(priv_dir, Config),
    Providers = [{listing_provider, provider_args(TestNames, lists:reverse(TestNames), "reversed")}],
    run_test_binary(Providers, OutputDir, ["list", "--output-dir", OutputDir]),
    {ok, Result} = file:read_file(filename:join(OutputDir, "result")),
    ListedTests = [json:decode(Line) || Line <- binary:split(Result, <<"\n">>, [global, trim_all])],
    ?assertEqual(
        [
            #{
                <<"testcase">> => TestName,
                <<"filter">> => TestName,
                <<"breakpoint">> => #{
                    <<"type">> => <<"function">>, <<"functionName">> => <<"provider_listing_SUITE:test_pass/1">>
                }
            }
         || TestName <- TestNames
        ],
        ListedTests
    ).

test_listing_filters_all(Config) ->
    OutputDir = proplists:get_value(priv_dir, Config),
    Providers = [{listing_provider, provider_args(test_names(), [], "empty")}, {list_test, []}],
    run_test_binary(Providers, OutputDir, ["list", "--output-dir", OutputDir]),
    ?assertEqual({ok, <<>>}, file:read_file(filename:join(OutputDir, "result"))),
    ?assertEqual({ok, term_to_binary([])}, file:read_file(filename:join(OutputDir, "empty"))).

-spec test_names() -> buck_ct_provider:test_names().
test_names() ->
    [~".test_pass", ~"outer.test_pass", ~"outer:inner_🫠.test_pass"].

-spec provider_args(Expected, Filtered, OutputFile) -> listing_provider:args() when
    Expected :: buck_ct_provider:test_names(),
    Filtered :: buck_ct_provider:test_names(),
    OutputFile :: string().
provider_args(Expected, Filtered, OutputFile) ->
    [#{expected => Expected, filtered => Filtered, output_file => OutputFile}].

-spec run_test_binary(Providers, OutputDir, Args) -> ok when
    Providers :: [{module(), listing_provider:args()}],
    OutputDir :: file:filename(),
    Args :: [string()].
run_test_binary(Providers, OutputDir, Args) ->
    {ok, [[ErlExec | ErlFlags]]} = init:get_argument(progname),
    CodePaths = [common_util:filename_all_to_filename(filename:absname(Path)) || Path <- code:get_path()],
    TestInfoFile = filename:join(OutputDir, "test_info.json"),
    ok = test_info:write_to_file(TestInfoFile, #test_info{
        dependencies = CodePaths,
        test_suite = common_util:unicode_characters_to_binary(filename:absname(code:which(provider_listing_SUITE))),
        config_files = [],
        providers = Providers,
        ct_opts = [],
        erl_cmd = [list_to_binary(ErlExec)],
        extra_flags = [],
        common_app_env = #{},
        artifact_annotation_mfa = fun artifact_annotations:default_annotation/1,
        raw_target = ~"test_binary_SUITE",
        trampolines = []
    }),
    Port = open_port({spawn_executable, filename:absname(ErlExec)}, [
        binary,
        exit_status,
        stderr_to_stdout,
        {args,
            ErlFlags ++ ["-pa" | CodePaths] ++
                [
                    "-noshell",
                    "-s",
                    "test_binary",
                    "main",
                    "-extra",
                    "--test-info-file",
                    TestInfoFile
                ] ++ Args}
    ]),
    collect_port_output(Port, <<>>).

-spec collect_port_output(Port, Output) -> ok when
    Port :: port(),
    Output :: binary().
collect_port_output(Port, Output) ->
    receive
        {Port, {data, Data}} -> collect_port_output(Port, <<Output/binary, Data/binary>>);
        {Port, {exit_status, 0}} -> ok;
        {Port, {exit_status, Status}} -> ct:fail({listing_failed, Status, Output})
    after 30000 ->
        port_close(Port),
        ct:fail({listing_timeout, Output})
    end.
