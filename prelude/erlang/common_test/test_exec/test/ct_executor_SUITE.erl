%% Copyright (c) Meta Platforms, Inc. and affiliates.
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.
%%% % @format
-module(ct_executor_SUITE).
-typing([eqwalizer]).

-export([all/0]).

-export([
    test_ct_run_exit_code/1,
    test_preload_app_file_atoms/1,
    test_split_args/1
]).

% elp:ignore WA003 (better_assertions) - Open Source
-include_lib("stdlib/include/assert.hrl").

all() ->
    [
        test_ct_run_exit_code,
        test_split_args,
        test_preload_app_file_atoms
    ].

test_ct_run_exit_code(_Config) ->
    ?assertEqual(0, ct_executor:ct_run_exit_code({1, 2, {3, 4}})),
    ?assertEqual(2, ct_executor:ct_run_exit_code(self())),
    ?assertEqual(2, ct_executor:ct_run_exit_code({error, test_failure})).

test_split_args(_Config) ->
    ?assertEqual(
        {
            [
                {output_dir, ""},
                {providers, [something]},
                {suite, a_suite},
                {common_app_env, #{<<"raw_target">> => <<"target">>}}
            ],
            [{dir, ""}, {suite, a_suite}, {group, a_group}]
        },
        ct_executor:split_args([
            {output_dir, ""},
            {providers, [something]},
            {suite, a_suite},
            {common_app_env, #{<<"raw_target">> => <<"target">>}},
            ct_args,
            {dir, ""},
            {suite, a_suite},
            {group, a_group}
        ])
    ),
    ?assertEqual(
        {[{output_dir, ""}, {providers, [something]}, {suite, a_suite}], []},
        ct_executor:split_args([{output_dir, ""}, {providers, [something]}, {suite, a_suite}, ct_args])
    ),
    ?assertEqual(
        {[], [{dir, ""}, {suite, a_suite}, {group, a_group}]},
        ct_executor:split_args([ct_args, {dir, ""}, {suite, a_suite}, {group, a_group}])
    ),
    ?assertEqual({[], []}, ct_executor:split_args([ct_args])).

test_preload_app_file_atoms(Config) ->
    PrivDir = proplists:get_value(priv_dir, Config),
    Suffix = integer_to_binary(erlang:unique_integer([positive])),
    AppName = <<"ct_executor_preload_app_", Suffix/binary>>,
    EnvName = <<"ct_executor_preload_env_", Suffix/binary>>,
    EnvValue = <<"ct_executor_preload_value_", Suffix/binary>>,
    AppFile = filename:join(PrivDir, binary_to_list(<<AppName/binary, ".app">>)),
    ok = file:write_file(AppFile, [
        "{application,",
        AppName,
        ",[{env,[{",
        EnvName,
        ",",
        EnvValue,
        "}]}]}.\n"
    ]),
    ?assertError(badarg, binary_to_existing_atom(EnvValue, utf8)),

    ok = ct_executor:preload_app_file_atoms(AppFile),

    ?assertEqual(EnvValue, atom_to_binary(binary_to_existing_atom(EnvValue, utf8), utf8)).
