%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(list_test).
-compile(warn_missing_spec_all).

-export([list_tests/2]).

-include_lib("common/include/tpx_records.hrl").

-import(common_util, [unicode_characters_to_binary/1]).

-type ct_groupname() :: ct_suite:ct_groupname().
-type ct_testname() :: ct_suite:ct_testname().
-type suite() :: module().

%% coming from the output of the group/0 method.
%% See https://www.erlang.org/doc/man/ct_suite.html#Module:groups-0 for the upstream type.
-type groups_output() :: [ct_group_def()].
-type ct_group_def() ::
    {ct_groupname(), ct_group_props(), [ct_group_content()]}
    | {ct_groupname(), [ct_group_content()]}.
-type ct_group_content() :: ct_testname() | ct_group_def() | {group, ct_groupname()} | ct_testcase_ref().

%% coming from the output of the all/0 method.
%% See https://www.erlang.org/doc/man/ct_suite.html#Module:all-0 for the upstream type.
-type all_output() :: [ct_test_def()].
-type ct_test_def() :: ct_testname() | ct_group_ref() | ct_testcase_ref().
-type ct_testcase_ref() :: {testcase, ct_testname(), ct_testcase_repeat_prop()}.
-type ct_testcase_repeat_prop() :: [
    {repeat, ct_test_repeat()}
    | {repeat_until_ok, ct_test_repeat()}
    | {repeat_until_fail, ct_test_repeat()}
].
-type ct_group_ref() ::
    {group, ct_groupname()}
    | {group, ct_groupname(), ct_group_props_ref()}
    | {group, ct_groupname(), ct_group_props_ref(), ct_subgroups_def()}.
-type ct_group_props_ref() :: ct_group_props() | default.
-type ct_group_props() ::
    [
        parallel
        | sequence
        | shuffle
        | {shuffle, Seed :: {integer(), integer(), integer()}}
        | {ct_group_repeat_type(), ct_test_repeat()}
    ].
-type ct_group_repeat_type() ::
    repeat
    | repeat_until_all_ok
    | repeat_until_all_fail
    | repeat_until_any_ok
    | repeat_until_any_fail.
-type ct_test_repeat() :: integer() | forever.
-type ct_subgroups_def() ::
    {ct_groupname(), ct_group_props_ref()}
    | {ct_groupname(), ct_group_props_ref(), ct_subgroups_def()}.

%% ------ Public Function --------

-doc """
Generates a list of tests, suitable for test runners like TPX
""".
-spec list_tests(Suite, Hooks) -> #test_spec_test_case{} when
    Suite :: suite(),
    Hooks :: [module()].
list_tests(Suite, Hooks) ->
    TestCases = list_test_spec(Suite, Hooks),
    throw_if_duplicate(TestCases),
    #test_spec_test_case{
        suite = atom_to_binary(Suite),
        testcases = TestCases
    }.

%% -------------- Internal functions ----------------
%%
%%
-spec throw_if_duplicate([#test_spec_test_info{}]) -> ok.
throw_if_duplicate(TestCaseInfos) ->
    throw_if_duplicate(sets:new([{version, 2}]), TestCaseInfos).

-spec throw_if_duplicate(sets:set(binary()), [#test_spec_test_info{}]) -> ok.
throw_if_duplicate(_, []) ->
    ok;
throw_if_duplicate(TestNameSet, [TestCaseInfo | Tail]) ->
    TestCaseName = TestCaseInfo#test_spec_test_info.name,
    case sets:is_element(TestCaseName, TestNameSet) of
        true ->
            throw({found_duplicate_test, TestCaseInfo});
        false ->
            throw_if_duplicate(sets:add_element(TestCaseName, TestNameSet), Tail)
    end.

-doc """
Test that all the tests in the list are exported.
""".
-spec test_exported_test(suite(), ct_testname()) -> error | ok.
test_exported_test(Suite, Test) ->
    case erlang:function_exported(Suite, Test, 1) of
        false ->
            case erlang:function_exported(Suite, '$handle_undefined_function', 2) of
                true ->
                    ok;
                false ->
                    error(
                        {invalid_test,
                            io_lib:format(
                                "The test ~ts has been discovered while recursively exploring all/0, " ++
                                    "groups/0 but is not an exported method of arity 1",
                                [Test]
                            )}
                    )
            end;
        true ->
            ok
    end.

-spec load_hooks([module()]) -> ok.
load_hooks(Hooks) ->
    ok = code:ensure_modules_loaded(Hooks).

%% We extract the call to the groups() method so that we can type it.
-spec suite_groups(suite(), [module()]) -> groups_output().
suite_groups(Suite, Hooks) ->
    GroupDef =
        case erlang:function_exported(Suite, groups, 0) of
            true -> Suite:groups();
            false -> []
        end,
    lists:foldl(
        fun(Hook, CurrGroupDef) ->
            case erlang:function_exported(Hook, post_groups, 2) of
                true ->
                    Hook:post_groups(Suite, CurrGroupDef);
                false ->
                    CurrGroupDef
            end
        end,
        GroupDef,
        Hooks
    ).

-spec suite_all(suite(), [module()], groups_output()) -> all_output().
suite_all(Suite, Hooks, GroupsDef) ->
    TestsDef = Suite:all(),
    lists:foldl(
        fun(Hook, CurrTestsDef) ->
            case erlang:function_exported(Hook, post_all, 3) of
                true ->
                    Hook:post_all(Suite, CurrTestsDef, GroupsDef);
                false ->
                    CurrTestsDef
            end
        end,
        TestsDef,
        Hooks
    ).

-spec list_test([ct_test_def() | ct_group_content()], [ct_groupname()], groups_output(), suite()) ->
    [#test_spec_test_info{}].
list_test(Node, Groups, SuiteGroups, Suite) ->
    lists:foldl(
        fun
            (Test, ListTestsAcc) when is_atom(Test) ->
                [test_case_info(Suite, Groups, Test) | ListTestsAcc];
            ({testcase, Test}, ListTestsAcc) when is_atom(Test) ->
                [test_case_info(Suite, Groups, Test) | ListTestsAcc];
            ({testcase, TestName, _Properties}, ListTestsAcc) when is_atom(TestName) ->
                [test_case_info(Suite, Groups, TestName) | ListTestsAcc];
            (Group, ListTestsAcc) ->
                lists:append(list_group(Group, Groups, SuiteGroups, Suite), ListTestsAcc)
        end,
        [],
        Node
    ).

%% case where the format of the group is {group, GroupName}, then we need to
%% look for the specifications of the group from the groups() method.
-spec list_group(ct_group_ref() | ct_group_def(), [ct_groupname()], groups_output(), suite()) ->
    [#test_spec_test_info{}].
list_group({group, Group}, Groups, SuiteGroups, Suite) when is_atom(Group) ->
    list_sub_group(Group, Groups, SuiteGroups, Suite);
%% case {group, GroupName, Properties}, similar as above
list_group({group, Group, _}, Groups, SuiteGroups, Suite) when is_atom(Group) ->
    list_sub_group(Group, Groups, SuiteGroups, Suite);
%% case {group, GroupName, Properties, SubGroupProperties},
%% similar_as_above.
list_group({group, Group, _, _}, Groups, SuiteGroups, Suite) ->
    list_sub_group(Group, Groups, SuiteGroups, Suite);
list_group(GroupDef, Groups, SuiteGroups, Suite) ->
    list_group_def(GroupDef, Groups, SuiteGroups, Suite).

-spec list_group_def(ct_group_def(), [ct_groupname()], groups_output(), suite()) -> [#test_spec_test_info{}].
%% case {GroupName, SubGroupTests}, then we need to look for the specification of the group
%% from the groups() method as above
list_group_def({Group, SubGroupTests}, Groups, SuiteGroups, Suite) ->
    Groups1 = lists:append(Groups, [Group]),
    list_test(SubGroupTests, Groups1, SuiteGroups, Suite);
%% case {GroupName, Properties, SubGroupsAndTests},
%% then in this case we explore the SubGroupsAndTests
list_group_def({Group, _, SubGroupTests}, Groups, SuiteGroups, Suite) ->
    Groups1 = lists:append(Groups, [Group]),
    list_test(SubGroupTests, Groups1, SuiteGroups, Suite).

-doc """
Makes use of the output from the groups/0 method to get the tests and subgroups
of the group name given as input
""".
-spec list_sub_group(ct_groupname(), [ct_groupname()], groups_output(), suite()) -> [#test_spec_test_info{}].
list_sub_group(Group, Groups, SuiteGroups, Suite) when is_list(SuiteGroups) ->
    TestsAndGroups =
        case lists:keyfind(Group, 1, SuiteGroups) of
            {Group, TestsDef} when is_list(TestsDef) -> TestsDef;
            {Group, _, TestsDef} when is_list(TestsDef) -> TestsDef;
            false -> error({invalid_group, Suite, Group});
            GroupSpec -> error({bad_group_spec, GroupSpec})
        end,
    Groups1 = lists:append(Groups, [Group]),
    list_test(TestsAndGroups, Groups1, SuiteGroups, Suite).

-spec test_case_info(suite(), [ct_groupname()], ct_testname()) -> #test_spec_test_info{}.
test_case_info(Suite, Groups, Test) ->
    ok = test_exported_test(Suite, Test),
    ListPeriodGroups = lists:join(":", [atom_to_list(Group) || Group <:- Groups]),
    Name = unicode_characters_to_binary(io_lib:format("~ts.~ts", [ListPeriodGroups, Test])),
    #test_spec_test_info{
        name = Name,
        filter = Name,
        breakpoint = {Suite, Test, 1}
    }.

-spec list_test_spec(Suite, Hooks) -> [#test_spec_test_info{}] when
    Suite :: suite(),
    Hooks :: [module()].
list_test_spec(Suite, Hooks) ->
    ok = load_hooks(Hooks),
    {module, Suite} = code:ensure_loaded(Suite),
    GroupsDef = suite_groups(Suite, Hooks),
    AllResult = suite_all(Suite, Hooks, GroupsDef),
    lists:reverse(list_test(AllResult, [], GroupsDef, Suite)).
