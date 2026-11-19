%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format
-module(common_util).
-compile(warn_missing_spec_all).

-export([
    unicode_characters_to_list/1,
    unicode_characters_to_binary/1,

    qualified_name/2,
    parse_test_name/2,

    filename_all_to_filename/1,

    get_env/1,
    set_env/2
]).

-include_lib("common/include/buck_ct_records.hrl").

-spec unicode_characters_to_list(unicode:chardata()) -> string().
unicode_characters_to_list(CharData) ->
    case unicode:characters_to_list(CharData) of
        R when not is_tuple(R) -> R
    end.

-doc """
Gets the name for a testcase in a given group-path
The groups order expected here is [leaf_group, ...., root_group]
""".
-spec qualified_name(Groups, TestCase) -> string() when
    Groups :: [atom()],
    TestCase :: string() | atom().
qualified_name(Groups, TestCase) ->
    StringGroups = [atom_to_list(Group) || Group <- Groups],
    JoinedGroups = string:join(lists:reverse(StringGroups), ":"),
    Raw = io_lib:format("~ts.~ts", [JoinedGroups, TestCase]),
    unicode_characters_to_list(Raw).

-doc """
Parse the test name, and decompose it into the test, group and suite atoms
""".
-spec parse_test_name(string(), atom()) -> #ct_test{}.
parse_test_name(Test, Suite) ->
    [Groups0, TestName] = string:split(Test, ".", all),
    Groups1 =
        case Groups0 of
            [] -> [];
            _ -> string:split(Groups0, ":", all)
        end,
    Groups = [list_to_atom(GroupStr) || GroupStr <:- Groups1],
    #ct_test{
        suite = Suite,
        groups = Groups,
        test_name = list_to_atom(TestName),
        canonical_name = Test
    }.

-spec unicode_characters_to_binary(unicode:chardata()) -> binary().
unicode_characters_to_binary(Chars) ->
    case unicode:characters_to_binary(Chars) of
        Bin when is_binary(Bin) -> Bin
    end.

-spec filename_all_to_filename(file:filename_all()) -> file:filename().
filename_all_to_filename(Filename) when is_binary(Filename) ->
    unicode_characters_to_list(Filename);
filename_all_to_filename(Filename) ->
    Filename.

%% Accessors for the `common` application's environment. Kept here so that
%% callers in other apps (e.g. `ct_executor` in the `test_exec` app) do not read
%% the `common` app's env directly across an app boundary (W0011).
-spec get_env(atom()) -> undefined | {ok, dynamic()}.
get_env(Key) ->
    application:get_env(common, Key).

-spec set_env(atom(), term()) -> ok.
set_env(Key, Value) ->
    application:set_env(common, Key, Value).
