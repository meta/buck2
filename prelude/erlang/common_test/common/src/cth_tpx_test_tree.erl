%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(cth_tpx_test_tree).
-compile([warn_missing_spec_all]).

%% Public API
-export([
    new_node/1,
    new_leaf/1,

    register_result/4,
    collect_results/3
]).

-export_type([
    tree/0,
    tree_node/0,
    test_leaf/0,

    name/0,

    method_result/0,
    collected_method_result/0,
    collected_result/0,
    outcome/0,

    group_path/0,
    method_id/0
]).

-include("method_ids.hrl").

-import(common_util, [unicode_characters_to_list/1]).

-type tree() :: test_leaf() | tree_node().

-type option(Type) :: Type | none.

-type name() :: string() | atom().

-type tree_node() :: #{
    name := name(),
    type := node,
    init_method := option(method_result()),
    end_method := option(method_result()),
    test_cases := #{name() => test_leaf()},
    sub_groups := #{name() => tree_node()}
}.

-type test_leaf() :: #{
    name := name(),
    type := leaf,
    init_method := option(method_result()),
    end_method := option(method_result()),
    main_method := option(method_result())
}.

-type method_result() :: #{
    name := name(),
    start_time => float(),
    end_time => float(),
    start_progress_marker => binary(),
    end_progress_marker => binary(),
    outcome := outcome(),
    details := unicode:chardata()
}.

-type collected_method_result() :: #{
    name := name(),
    start_time => float(),
    end_time => float(),
    outcome := outcome(),
    details := unicode:chardata(),
    std_out := unicode:chardata()
}.

-type outcome() ::
    passed | failed | timeout | skipped | omitted | infra_failure.

-type group_path() :: [atom()].

-type collected_result() :: #{
    inits := [collected_method_result()],
    main := collected_method_result(),
    ends := [collected_method_result()]
}.

%% Tree creation and update

-doc """
Creates a new node
""".
-spec new_node(Name :: name()) -> tree_node().
new_node(Name) ->
    #{
        name => Name,
        type => node,
        init_method => none,
        end_method => none,
        test_cases => #{},
        sub_groups => #{}
    }.

-doc """
Creates a new leaf
""".
-spec new_leaf(Name :: name()) -> test_leaf().
new_leaf(Name) ->
    #{
        name => Name,
        type => leaf,
        init_method => none,
        end_method => none,
        main_method => none
    }.

-doc """
Puts the test result inside the tree.
""".
-spec register_result(tree_node(), method_result(), group_path(), method_id()) -> tree_node().
register_result(TreeResult, Result, Groups, MethodId) ->
    insert_result(TreeResult, Result, lists:reverse(Groups), MethodId).

-doc """
Inserts the method_result inside the tree.
""".
-spec insert_result(Node, Result, ReversedPath, MethodId) -> Node when
    Node :: tree_node(),
    Result :: method_result(),
    ReversedPath :: group_path(),
    MethodId :: method_id().
insert_result(TreeNode, ResultTest, [Group | Groups], MethodId) ->
    #{sub_groups := Children} = TreeNode,
    GroupNode = maps:get(Group, Children, new_node(Group)),
    NewChildren = Children#{Group => insert_result(GroupNode, ResultTest, Groups, MethodId)},
    TreeNode#{sub_groups => NewChildren};
insert_result(TreeNode, ResultTest, [], MethodId) ->
    case MethodId of
        Init when Init =:= ?INIT_PER_SUITE; Init =:= ?INIT_PER_GROUP ->
            TreeNode#{init_method => ResultTest};
        End when End =:= ?END_PER_SUITE; End =:= ?END_PER_GROUP ->
            TreeNode#{end_method => ResultTest};
        {NameCase, Phase} ->
            #{test_cases := Cases} = TreeNode,
            TestLeaf = maps:get(NameCase, Cases, new_leaf(NameCase)),
            NewTestLeaf =
                case Phase of
                    ?INIT_PER_TESTCASE ->
                        TestLeaf#{init_method => ResultTest};
                    ?MAIN_TESTCASE ->
                        Prev = maps:get(main_method, TestLeaf, none),
                        TestLeaf#{main_method => preserve_test_timing(Prev, ResultTest)};
                    ?END_PER_TESTCASE ->
                        TestLeaf#{end_method => ResultTest}
                end,
            TreeNode#{test_cases => Cases#{NameCase => NewTestLeaf}}
    end.

%% When `?MAIN_TESTCASE` is registered a second time (e.g. a testcase returning `{fail, Reason}`
%% due to https://github.com/erlang/otp/issues/11647 or a `post_end_per_testcase` hook returning
%% `{fail, Reason}`, where `post_end_per_testcase` arrives with `{tc_status, ok}`, records
%% `?MAIN_TESTCASE` as `passed` consuming `start_info`, and `on_tc_fail` subsequently overwrites
%% `?MAIN_TESTCASE`), preserve `start_time`, `end_time`, and stdout progress markers from the
%% earlier registration if the new registration lacks them.
-spec preserve_test_timing(option(method_result()), method_result()) -> method_result().
preserve_test_timing(none, New) ->
    New;
preserve_test_timing(Prev, New) ->
    maps:merge(
        maps:with([start_time, end_time, start_progress_marker, end_progress_marker], Prev),
        New
    ).

%% Collecting results

-doc """
Provides a result for the RequestedResults based on the collected results.
The format of the requested_results is a map from a list of groups to the list of test_cases that are sub-cases from the last group from the list.
""".
-spec collect_results(TreeResult, RequestedResults, CollectedStdOut) -> [collected_result()] when
    TreeResult :: tree(),
    RequestedResults :: #{group_path() => [atom()]},
    CollectedStdOut :: ct_stdout:collected_stdout().
collect_results(TreeResult, RequestedResults, CollectedStdOut) ->
    [
        collect_result(TreeResult, Groups, CaseRequest, CollectedStdOut)
     || Groups := CasesRequests <- RequestedResults,
        CaseRequest <- CasesRequests
    ].

-doc """
Provides a result for a given specific requested_result.
""".
-spec collect_result(TreeResult, Groups, TestCase, CollectedStdOut) -> collected_result() when
    TreeResult :: tree(),
    Groups :: group_path(),
    TestCase :: atom(),
    CollectedStdOut :: ct_stdout:collected_stdout().
collect_result(TreeResult, Groups, TestCase, CollectedStdOut) ->
    QualifiedName = common_util:qualified_name(lists:reverse(Groups), TestCase),
    LeafResult = collect_result(TreeResult, [], [], Groups, TestCase, QualifiedName, CollectedStdOut),
    #{ends := EndsResults, main := MainResult} = LeafResult,
    MainResultWithEndFailure = report_end_failure(EndsResults, MainResult),
    LeafResult#{main => MainResultWithEndFailure}.

-spec report_end_failure(MethodResults, ResultAcc) -> Result when
    MethodResults :: [collected_method_result()],
    ResultAcc :: collected_method_result(),
    Result :: collected_method_result().
report_end_failure([], ResultAcc) ->
    ResultAcc;
report_end_failure([#{outcome := passed} | Rest], ResultAcc) ->
    report_end_failure(Rest, ResultAcc);
report_end_failure([#{outcome := skipped} | Rest], ResultAcc) ->
    report_end_failure(Rest, ResultAcc);
report_end_failure(
    [#{outcome := EndOutcome, details := EndDetails, name := EndName} | Rest],
    #{name := TestName, outcome := ResultOutcome, details := ResultDetails} = ResultAcc
) ->
    MergedOutcome = merge_outcome(EndOutcome, ResultOutcome),
    EndFailedDetails = io_lib:format(~"~tp ~tp because ~tp failed with~n~ts", [
        TestName, MergedOutcome, EndName, EndDetails
    ]),

    MergedDetails =
        case ResultOutcome of
            passed ->
                EndFailedDetails;
            _ ->
                io_lib:format("~ts~n~n~ts", [ResultDetails, EndFailedDetails])
        end,
    report_end_failure(Rest, ResultAcc#{outcome => MergedOutcome, details => MergedDetails}).

%% A genuine failure or timeout outranks an infra failure, so that a real breakage is
%% never masked behind an infra label (tpx applies the same precedence when it
%% reconciles a retry against its previous attempt).
-spec merge_outcome(outcome(), outcome()) -> outcome().
merge_outcome(failed, _) -> failed;
merge_outcome(_, failed) -> failed;
merge_outcome(timeout, _) -> timeout;
merge_outcome(_, timeout) -> timeout;
merge_outcome(infra_failure, _) -> infra_failure;
merge_outcome(_, infra_failure) -> infra_failure;
merge_outcome(skipped, _) -> skipped;
merge_outcome(_, skipped) -> skipped;
merge_outcome(passed, Other) -> Other.

-doc """
Collects all the inits / ends methods results linked to a requested_result.
""".
-spec collect_result(Node, Inits, Ends, Groups, TestCase, QualifiedName, CollectedStdOut) -> collected_result() when
    Node :: tree(),
    Inits :: [collected_method_result()],
    Ends :: [collected_method_result()],
    Groups :: group_path(),
    TestCase :: atom(),
    QualifiedName :: string(),
    CollectedStdOut :: ct_stdout:collected_stdout().
collect_result(Node, Inits, Ends, Groups, TestCase, QualifiedName, CollectedStdOut) ->
    {NewInits, OptMain, NewEnds} = collect_node(Node, Inits, Ends, QualifiedName, CollectedStdOut),
    case OptMain of
        none ->
            {Child, NewGroups} = get_child(Node, Groups, TestCase),
            collect_result(Child, NewInits, NewEnds, NewGroups, TestCase, QualifiedName, CollectedStdOut);
        MainResult ->
            #{inits => lists:reverse(NewInits), main => MainResult, ends => NewEnds}
    end.

-spec get_child(Node, Groups, TestCase) -> {tree(), group_path()} when
    Node :: tree(),
    Groups :: group_path(),
    TestCase :: atom().
get_child(#{sub_groups := SubGroups}, [Group | Groups], _TestCase) ->
    {maps:get(Group, SubGroups, new_node(Group)), Groups};
get_child(#{test_cases := TestCases}, [], TestCase) ->
    {maps:get(TestCase, TestCases, new_leaf(TestCase)), []}.

-doc """
Collect the results from init_testcase, end_testcase and the main testcase for a given requested result.
Proceeds with some additional logic if the result is missing or skipped.
""".
-spec collect_node(Tree, Inits, Ends, QualName, CollectedStdOut) ->
    {
        NewInits :: [collected_method_result()],
        MainResult :: option(collected_method_result()),
        NewEnds :: [collected_method_result()]
    }
when
    Tree :: tree(),
    Inits :: [collected_method_result()],
    Ends :: [collected_method_result()],
    QualName :: string(),
    CollectedStdOut :: ct_stdout:collected_stdout().
collect_node(
    #{type := leaf} = TestLeaf,
    Inits,
    Ends,
    QualifiedName,
    CollectedStdOut
) ->
    #{init_method := OptMethodInit, end_method := OptMethodEnd, main_method := OptMethodMain} = TestLeaf,
    {NewInits, NewEnds} = update_inits_ends(Inits, Ends, OptMethodInit, OptMethodEnd, CollectedStdOut),
    InitsPassed = lists:all(
        fun
            (#{outcome := failed}) -> false;
            (#{outcome := timeout}) -> false;
            (#{outcome := infra_failure}) -> false;
            (_) -> true
        end,
        NewInits
    ),
    %% Do NOT try to collect a result when one of the inits failed
    MainResult =
        case {InitsPassed, OptMethodMain} of
            {false, _} ->
                get_missing_result(NewInits, QualifiedName, CollectedStdOut);
            {true, none} ->
                get_missing_result(NewInits, QualifiedName, CollectedStdOut);
            {true, Result} ->
                case maps:get(outcome, Result) of
                    skipped -> handle_skipped_result(NewInits, Result, CollectedStdOut);
                    _ -> to_collected_method_result(Result, CollectedStdOut)
                end
        end,
    {NewInits, MainResult, NewEnds};
collect_node(
    #{type := node} = TreeNode,
    Inits,
    Ends,
    _QualifiedName,
    CollectedStdOut
) ->
    #{init_method := OptMethodInit, end_method := OptMethodEnd} = TreeNode,
    {NewInits, NewEnds} = update_inits_ends(Inits, Ends, OptMethodInit, OptMethodEnd, CollectedStdOut),
    {NewInits, none, NewEnds}.

-spec update_inits_ends(Inits, Ends, MethInit, MethEnd, CollectedStdOut) ->
    {NewInits :: [collected_method_result()], NewEnds :: [collected_method_result()]}
when
    Inits :: [collected_method_result()],
    Ends :: [collected_method_result()],
    MethInit :: option(method_result()),
    MethEnd :: option(method_result()),
    CollectedStdOut :: ct_stdout:collected_stdout().
update_inits_ends(Inits, Ends, OptMethodInit, OptMethodEnd, CollectedStdOut) ->
    NewInits =
        case OptMethodInit of
            none -> Inits;
            _ -> [to_collected_method_result(OptMethodInit, CollectedStdOut) | Inits]
        end,
    NewEnds =
        case OptMethodEnd of
            none -> Ends;
            _ -> [to_collected_method_result(OptMethodEnd, CollectedStdOut) | Ends]
        end,
    {NewInits, NewEnds}.

-doc """
Creates a method_result for a requested method for which no result was registered.
Attempts to locate if one of the inits is responsible for the missing result.
""".
-spec get_missing_result(Inits, QualifiedName, CollectedStdOut) -> collected_method_result() when
    Inits :: [collected_method_result()],
    QualifiedName :: string(),
    CollectedStdOut :: ct_stdout:collected_stdout().
get_missing_result(Inits, QualifiedName, CollectedStdOut) ->
    MainResult =
        #{
            name => unicode_characters_to_list(
                io_lib:format("~ts.[main_testcase]", [QualifiedName])
            ),
            outcome => failed,
            details => ~"no results for this test were recorded"
        },
    handle_skipped_result(Inits, MainResult, CollectedStdOut).

-doc """
Attributes a test case that left no result of its own, or that reports itself as
`skipped`, to the init that is responsible for it.

`Inits` runs innermost first, and the first one that did not pass is the one blamed: the
case takes that init's outcome, with a message naming the init and quoting its details.
`failed`, `timeout`, `infra_failure`, and `skipped` therefore carry through as
themselves; an `omitted` init is blamed as a failure. When every init passed,
`MainResult` stands.
""".
-spec handle_skipped_result(Inits, MainResult, CollectedStdOut) -> collected_method_result() when
    Inits :: [collected_method_result()],
    MainResult :: method_result(),
    CollectedStdOut :: ct_stdout:collected_stdout().
handle_skipped_result([], MainResult, CollectedStdOut) ->
    to_collected_method_result(MainResult, CollectedStdOut);
handle_skipped_result([Init | Inits], MainResult = #{name := Name}, CollectedStdOut) ->
    InitStdOut = io_lib:format(~"~ts stdout: ~ts", [maps:get(name, Init), maps:get(std_out, Init)]),
    case maps:get(outcome, Init) of
        failed ->
            #{
                name => Name,
                outcome => failed,
                details =>
                    io_lib:format(
                        ~"Failed because init ~ts failed, with error message:\n ~ts",
                        [maps:get(name, Init), maps:get(details, Init)]
                    ),

                std_out => InitStdOut
            };
        timeout ->
            #{
                name => Name,
                outcome => timeout,
                details =>
                    io_lib:format(
                        ~"Timed-out because init ~ts timed-out, with error message:\n ~ts",
                        [maps:get(name, Init), maps:get(details, Init)]
                    ),

                std_out => InitStdOut
            };
        infra_failure ->
            #{
                name => Name,
                outcome => infra_failure,
                details =>
                    io_lib:format(
                        ~"Infra failure because init ~ts hit an infra failure, with error message:\n ~ts",
                        [maps:get(name, Init), maps:get(details, Init)]
                    ),

                std_out => InitStdOut
            };
        passed ->
            handle_skipped_result(Inits, MainResult, CollectedStdOut);
        skipped ->
            #{
                name => Name,
                outcome => skipped,
                details =>
                    io_lib:format(
                        ~"Skipped because init ~ts was skipped, with reason:\n ~ts",
                        [maps:get(name, Init), maps:get(details, Init)]
                    ),

                std_out => InitStdOut
            };
        omitted ->
            #{
                name => Name,
                outcome => failed,
                details =>
                    io_lib:format(
                        ~"Failed because init ~ts was omitted, with error message:\n ~ts",
                        [maps:get(name, Init), maps:get(details, Init)]
                    ),

                std_out => InitStdOut
            }
    end.

-spec to_collected_method_result(MethodResult, CollectedStdOut) -> collected_method_result() when
    MethodResult :: method_result(),
    CollectedStdOut :: ct_stdout:collected_stdout().
to_collected_method_result(MethodResult, CollectedStdOut) ->
    StdOut =
        case MethodResult of
            #{start_progress_marker := Marker} ->
                case maps:get(Marker, CollectedStdOut) of
                    StdOutBin when is_binary(StdOutBin) -> StdOutBin;
                    {truncated, StdOutBin} ->
                        io_lib:format(
                            "NOTICE: stdout was truncated! See ~ts for the full logs of the suite.~n~n...~ts", [
                                ct_stdout:filename(), StdOutBin
                            ]
                        )
                end;
            _ ->
                ~""
        end,
    MethodResult1 = maps:without([start_progress_marker, end_progress_marker], MethodResult),
    MethodResult1#{std_out => StdOut}.
