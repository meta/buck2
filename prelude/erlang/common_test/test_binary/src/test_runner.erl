%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(test_runner).
-compile(warn_missing_spec_all).

-export([run_tests/6]).

-include_lib("common/include/tpx_records.hrl").
-include_lib("common/include/buck_ct_records.hrl").
-include_lib("kernel/include/logger.hrl").

-import(common_util, [unicode_characters_to_list/1, unicode_characters_to_binary/1]).

-define(DEFAULT_OUTPUT_FORMAT, json).
-define(MAX_STDOUT_BYTES_PER_TESTCASE, (16 * 1024)).

-spec run_tests(Tests, TestInfo, OutputDir, Listing, Timeout, StdoutStreaming) -> ok when
    Tests :: [string()],
    TestInfo :: #test_info{},
    OutputDir :: file:filename_all(),
    Listing :: #test_spec_test_case{},
    Timeout :: timeout(),
    StdoutStreaming :: output_to_stdout | no_output_to_stdout.
run_tests(Tests, #test_info{} = TestInfo, OutputDir, Listing, Timeout, StdoutStreaming) ->
    check_ct_opts(TestInfo#test_info.ct_opts),
    Suite =
        case filename:basename(TestInfo#test_info.test_suite, ".beam") of
            SuiteBin when is_binary(SuiteBin) -> binary_to_atom(SuiteBin);
            SuiteStr when is_list(SuiteStr) -> list_to_atom(SuiteStr)
        end,
    StructuredTests = [common_util:parse_test_name(Test, Suite) || Test <- Tests],
    case StructuredTests of
        [] ->
            throw(no_tests_to_run);
        [_ | _] ->
            TestSpecFile = filename:join(OutputDir, "test_spec.spec"),
            OrderedTests = reorder_tests(StructuredTests, Listing),

            execute_test_suite(
                #test_env{
                    output_format = ?DEFAULT_OUTPUT_FORMAT,
                    suite = Suite,
                    tests = OrderedTests,
                    suite_path = TestInfo#test_info.test_suite,
                    output_dir = OutputDir,
                    dependencies = TestInfo#test_info.dependencies,
                    test_spec_file = TestSpecFile,
                    config_files = TestInfo#test_info.config_files,
                    providers = TestInfo#test_info.providers,
                    ct_opts = TestInfo#test_info.ct_opts,
                    common_app_env = TestInfo#test_info.common_app_env,
                    erl_cmd = TestInfo#test_info.erl_cmd,
                    extra_flags = TestInfo#test_info.extra_flags,
                    artifact_annotation_mfa = TestInfo#test_info.artifact_annotation_mfa,
                    raw_target = TestInfo#test_info.raw_target,
                    trampolines = TestInfo#test_info.trampolines,
                    timeout = Timeout,
                    ct_stdout_fingerprint = ct_stdout:make_fingerprint(),
                    ct_stdout_streaming = StdoutStreaming,
                    result_recipient = self()
                }
            )
    end.

-doc """
Prepare the test spec and run the test.
""".
-spec execute_test_suite(TestEnv) -> ok when
    TestEnv :: #test_env{}.
execute_test_suite(TestEnv) ->
    #test_env{
        suite = Suite,
        tests = Tests,
        suite_path = SuitePath,
        output_dir = OutputDir,
        test_spec_file = TestSpecFile,
        ct_opts = CtOpts,
        timeout = Timeout
    } = TestEnv,
    TestSpec = build_test_spec(
        Suite,
        Tests,
        filename:absname(filename:dirname(SuitePath)),
        OutputDir,
        CtOpts,
        TestEnv#test_env.ct_stdout_fingerprint
    ),
    FormattedSpec = [io_lib:format("~tp.~n", [Entry]) || Entry <- TestSpec],
    file:write_file(TestSpecFile, FormattedSpec, [raw, binary]),
    NewTestEnv = TestEnv#test_env{test_spec_file = TestSpecFile, ct_opts = CtOpts},
    try run_test(NewTestEnv, Timeout) of
        ok -> ok
    catch
        Class:Reason:StackTrace ->
            FailureTime = erlang:system_time(millisecond),
            ErrorMsg = io_lib:format("run_test failed due to ~ts\n", [
                erl_error:format_exception(Class, Reason, StackTrace)
            ]),
            ?LOG_ERROR(ErrorMsg),
            test_run_fail(NewTestEnv, ErrorMsg, #{}, FailureTime)
    end.

-spec run_test(TestEnv, Timeout) -> ok when
    TestEnv :: #test_env{},
    Timeout :: timeout().
run_test(TestEnv, Timeout) ->
    application:set_env(test_exec, test_env, TestEnv, [{persistent, true}]),
    case application:ensure_all_started(test_exec, temporary) of
        {ok, _Apps} ->
            Ref = erlang:monitor(process, test_exec_sup, []),
            receive
                {'DOWN', Ref, _Type, Object, Info} ->
                    FailureTime = erlang:system_time(millisecond),
                    test_run_fail(
                        TestEnv,
                        io_lib:format(
                            "unexpected exception in the buck2 Common Test runner:\n"
                            "                        application test_exec crashed (~tp ~tp) ~n",
                            [Object, Info]
                        ),
                        #{},
                        FailureTime
                    );
                {run_succeed, Result, ProgressMarkersOffsets} ->
                    ensure_test_exec_stopped(),
                    test_run_succeed(TestEnv, Result, ProgressMarkersOffsets);
                {run_failed, Result, ProgressMarkersOffsets} ->
                    FailureTime = erlang:system_time(millisecond),
                    ensure_test_exec_stopped(),
                    test_run_fail(TestEnv, Result, ProgressMarkersOffsets, FailureTime)
            after Timeout ->
                FailureTime = erlang:system_time(millisecond),
                ensure_test_exec_stopped(),
                ErrorMsg =
                    "\n************************\n"
                    "* the suite timed out. *\n"
                    "************************\n",
                test_run_timeout(TestEnv, ErrorMsg, FailureTime)
            end;
        {error, Reason} ->
            FailureTime = erlang:system_time(millisecond),
            ErrorMsg = io_lib:format("TextExec failed to start due to ~tp", [Reason]),

            ?LOG_ERROR(ErrorMsg),
            test_run_fail(TestEnv, ErrorMsg, #{}, FailureTime)
    end.

-spec ensure_test_exec_stopped() -> ok.
ensure_test_exec_stopped() ->
    {Pid, Monitor} = erlang:spawn_monitor(fun() -> application:stop(test_exec) end),
    receive
        {'DOWN', Monitor, process, Pid, _} -> ok
    after 5000 -> ok
    end.

-doc """
Provides result as specified by the tpx protocol when test failed to ran.
""".
-spec test_run_fail(TestEnv, Reason, ProgressMarkersOffsets, FailureTime) -> ok when
    TestEnv :: #test_env{},
    Reason :: unicode:chardata(),
    ProgressMarkersOffsets :: #{ct_stdout:progress_line() => ct_stdout:offset()},
    FailureTime :: integer().
test_run_fail(TestEnv, Reason, ProgressMarkersOffsets, FailureTime) ->
    provide_output_file(
        TestEnv,
        io_lib:format("Test failed to ran due to ~ts", [Reason]),
        failed,
        ProgressMarkersOffsets,
        FailureTime
    ).

-spec test_run_timeout(#test_env{}, string(), integer()) -> ok.
test_run_timeout(TestEnv, Reason, FailureTime) ->
    provide_output_file(TestEnv, Reason, timeout, #{}, FailureTime).

-doc """
Provides result as specified by the tpx protocol when test succeed to ran.
""".
-spec test_run_succeed(TestEnv, Reason, ProgressMarkersOffsets) -> ok when
    TestEnv :: #test_env{},
    Reason :: unicode:chardata(),
    ProgressMarkersOffsets :: #{ct_stdout:progress_line() => ct_stdout:offset()}.
test_run_succeed(TestEnv, Reason, ProgressMarkersOffsets) ->
    provide_output_file(TestEnv, Reason, passed, ProgressMarkersOffsets, 0).

-doc """
Provides result as specified by the tpx protocol.
""".
-spec provide_output_file(TestEnv, ResultExec, Status, ProgressMarkersOffsets, FailureTime) -> ok when
    TestEnv :: #test_env{},
    ResultExec :: unicode:chardata(),
    Status :: failed | passed | timeout,
    ProgressMarkersOffsets :: #{ct_stdout:progress_line() => ct_stdout:offset()},
    FailureTime :: integer().
provide_output_file(
    #test_env{
        output_dir = OutputDir,
        tests = Tests,
        artifact_annotation_mfa = ArtifactAnnotationFunction
    },
    ResultExec,
    Status,
    ProgressMarkersOffsets,
    FailureTime
) ->
    LogFile = test_logger:get_log_file(OutputDir, ct_executor),
    StdOutFile = ct_stdout:filename(OutputDir),
    LogFilesForCrashes = #{
        ct_executor_log => LogFile,
        ct_executor_stdout => StdOutFile
    },

    ResultsFile = filename:join(OutputDir, "result.json"),
    Results =
        case Status of
            failed ->
                collect_results_broken_run(
                    Tests, ~"internal crash", ResultExec, LogFilesForCrashes
                );
            timeout ->
                % Once the suite watchdog fires, group/suite teardown may not
                % have completed, so no testcase has an independent verdict.
                [];
            passed ->
                case file:read_file(ResultsFile, [raw]) of
                    {ok, JsonFile} ->
                        TreeResults = decode_erlang_term(JsonFile),
                        case TreeResults of
                            undefined ->
                                ErrorMsg =
                                    io_lib:format(
                                        ~"ct failed to produced results valid file ~tp", [
                                            ResultsFile
                                        ]
                                    ),
                                collect_results_broken_run(
                                    Tests, ErrorMsg, ResultExec, LogFilesForCrashes
                                );
                            _ ->
                                {ok, CollectedStdOut} = ct_stdout:collect_method_stdout(
                                    StdOutFile,
                                    ProgressMarkersOffsets,
                                    TreeResults,
                                    ?MAX_STDOUT_BYTES_PER_TESTCASE
                                ),
                                collect_results_fine_run(TreeResults, Tests, CollectedStdOut)
                        end;
                    {error, _Reason} ->
                        ErrorMsg = io_lib:format(~"ct failed to produced results file ~tp", [
                            ResultsFile
                        ]),
                        collect_results_broken_run(
                            Tests, ErrorMsg, ResultExec, LogFilesForCrashes
                        )
                end
        end,

    ok = test_result_output:write(OutputDir, Status, FailureTime, ResultExec, Results),
    test_artifact_directory:link_to_artifact_dir(
        StdOutFile, OutputDir, ArtifactAnnotationFunction
    ),
    test_artifact_directory:link_to_artifact_dir(
        test_logger:get_std_out(OutputDir, test_runner), OutputDir, ArtifactAnnotationFunction
    ),
    test_artifact_directory:prepare(OutputDir, Tests, ArtifactAnnotationFunction).

-spec decode_erlang_term(Bin :: binary()) -> dynamic().
decode_erlang_term(Bin) ->
    binary_to_term(Bin).

-spec trimmed_content_file(File) -> unicode:chardata() when
    File :: file:filename_all().
trimmed_content_file(File) ->
    case file:open(File, [read, raw, binary]) of
        {error, Reason} ->
            io_lib:format(~"No ~tp file found, reason ~tp ", [filename:basename(File), Reason]);
        {ok, FD} ->
            try
                case file:position(FD, {eof, -5000}) of
                    {error, _} ->
                        {ok, _} = file:position(FD, bof),
                        case file:read(FD, 5000) of
                            {ok, Data} -> Data;
                            eof -> io_lib:format(~"nothing to read from ~ts", [File])
                        end;
                    {ok, _} ->
                        {ok, EndOfFile} = file:read(FD, 5000),
                        io_lib:format(~"~ts~nFile truncated, see ~tp for full output", [
                            EndOfFile,
                            filename:basename(File)
                        ])
                end
            of
                Content -> Content
            after
                file:close(FD)
            end
    end.

-doc """
Provide tpx with a result when CT failed to provide results for tests.
""".
-spec collect_results_broken_run(Tests, ErrorMsg, ResultExec, RelevantLogFiles) ->
    [cth_tpx_test_tree:collected_result()]
when
    Tests :: [#ct_test{}],
    ErrorMsg :: unicode:chardata(),
    ResultExec :: unicode:chardata(),
    RelevantLogFiles :: #{
        ct_executor_log => file:filename_all(),
        ct_executor_stdout := file:filename_all()
    }.
collect_results_broken_run(Tests, ErrorMsg, ResultExec, RelevantLogFiles) ->
    #{ct_executor_stdout := StdOutFile} = RelevantLogFiles,
    TrimmedStdOut = trimmed_content_file(StdOutFile),
    StdOut =
        case RelevantLogFiles of
            #{ct_executor_log := ExecutorLogFile} ->
                TrimmedExecutorLog = trimmed_content_file(ExecutorLogFile),
                io_lib:format("ct_executor_log: ~ts ~nct_executor_stdout: ~ts", [TrimmedExecutorLog, TrimmedStdOut]);
            _ ->
                TrimmedStdOut
        end,

    FormattedErrorMsg = io_lib:format("~ts~n", [ErrorMsg]),
    [
        #{
            ends => [],
            inits => [],
            main => #{
                name => lists:flatten(
                    io_lib:format("~ts.[main_testcase]", [
                        % We need to reverse the list of groups as the method common_util:qualified_name expects them
                        % in the reverse order (as it is designed to be called when exploring the tree of results
                        % where we push at each time the group we are in, leading to them being in reverse order).
                        common_util:qualified_name(
                            lists:reverse(Test#ct_test.groups),
                            Test#ct_test.test_name
                        )
                    ])
                ),
                details =>
                    unicode_characters_to_list(
                        io_lib:format(
                            "~ts~ts ~n",
                            [FormattedErrorMsg, ResultExec]
                        )
                    ),
                outcome => failed,
                std_out => unicode_characters_to_list(StdOut)
            }
        }
     || Test <- Tests
    ].

-doc """
Provide the results from the tests as specified by tpx protocol, from the json file
provided by ct displaying results of all the tests ran.
""".
-spec collect_results_fine_run(TestResults, Tests, CollectedStdOut) -> [cth_tpx_test_tree:collected_result()] when
    TestResults :: cth_tpx_test_tree:tree_node(),
    Tests :: [#ct_test{}],
    CollectedStdOut :: ct_stdout:collected_stdout().
collect_results_fine_run(TreeResults, Tests, CollectedStdOut) ->
    cth_tpx_test_tree:collect_results(
        TreeResults, get_requested_tests_by_group_path(Tests), CollectedStdOut
    ).

-doc """
Splits the requested tests into consecutive runs of tests sharing a (sequence) of groups.
The list is [{[sequence of groups], [list of tests belonging to this sequence]}], in the order the
tests were requested, so a sequence returned to later gets a further entry. That is, if the sequence
is g1.t1, g2.t1, g1.t2, g1.t3, g2.t2, we produce:
[{g1,[t1]}, {g2,[t1]}, {g1,[t2,t3]}, {g2,[t2]}]
The tests are expected in listing order, the depth first traversal of all/0 with groups/0 expanded
that reorder_tests/2 puts them in. The runs only follow the order the suite declares under that
precondition.
""".
-spec get_requested_tests(Tests) -> [{GroupPath, TestCases}] when
    Tests :: [#ct_test{}],
    GroupPath :: cth_tpx_test_tree:group_path(),
    TestCases :: [ct_suite:ct_testname()].
get_requested_tests(Tests) ->
    lists:foldr(
        fun(#ct_test{groups = Groups, test_name = TestName}, Runs) ->
            case Runs of
                [{Groups, TestNames} | Rest] -> [{Groups, [TestName | TestNames]} | Rest];
                _ -> [{Groups, [TestName]} | Runs]
            end
        end,
        [],
        Tests
    ).

-spec get_requested_tests_by_group_path(Tests) -> #{GroupPath => TestCases} when
    Tests :: [#ct_test{}],
    GroupPath :: cth_tpx_test_tree:group_path(),
    TestCases :: [ct_suite:ct_testname()].
get_requested_tests_by_group_path(Tests) ->
    lists:foldr(
        fun(#ct_test{groups = Groups, test_name = TestName}, Acc) ->
            maps:update_with(Groups, fun(TestNames) -> [TestName | TestNames] end, [TestName], Acc)
        end,
        #{},
        Tests
    ).

-doc """
Built the test_spec selecting the requested tests and
specifying the result output.
""".
-spec build_test_spec(Suite, Tests, TestDir, OutputDir, CtOpts, ProgressLineFingerprint) -> [term()] when
    Suite :: module(),
    Tests :: [#ct_test{}],
    TestDir :: file:filename_all(),
    OutputDir :: file:filename_all(),
    CtOpts :: [term()],
    ProgressLineFingerprint :: ct_stdout:fingerprint().
build_test_spec(Suite, Tests, TestDir0, OutputDir, CtOpts, ProgressLineFingerprint) ->
    TestDir = unicode_characters_to_list(TestDir0),
    ListGroupTest = get_requested_tests(Tests),
    SpecTests = lists:map(
        fun
            ({[], TopTests}) ->
                {cases, TestDir, Suite, TopTests};
            ({Groups, GroupTests}) ->
                GroupPath = [[{Group, []} || Group <- Groups]],
                {groups, TestDir, Suite, GroupPath, {cases, GroupTests}}
        end,
        ListGroupTest
    ),
    ResultOutput = filename:join(OutputDir, ~"result.json"),
    {TpxCtHook, CtOpts1} = getCtHook(CtOpts, ResultOutput, ProgressLineFingerprint),
    LogDir = set_up_log_dir(OutputDir),
    CtOpts2 = add_spec_if_absent(
        {auto_compile, false}, add_spec_if_absent({logdir, LogDir}, CtOpts1)
    ),
    SpecTests ++ [TpxCtHook] ++ CtOpts2.

-doc """
Collect all the ct_hooks entries provided by the user, and add the cth_tpx hook config.
""".
-spec getCtHook(CtOpts0, ResultOutput, ProgressLineFingerprint) -> {CtHooks, CtOpts1} when
    CtOpts0 :: [term()],
    ResultOutput :: file:filename_all(),
    ProgressLineFingerprint :: ct_stdout:fingerprint(),
    CtHooks :: {ct_hooks, [term()]},
    CtOpts1 :: [term()].
getCtHook(CtOpts0, ResultOutput, ProgressLineFingerprint) ->
    {CtHooksOpts, CtOpts1} = lists:partition(
        fun
            ({ct_hooks, _}) -> true;
            (_) -> false
        end,
        CtOpts0
    ),
    CtHooks0 = [CtHook || {ct_hooks, CtHooks = [_ | _]} <- CtHooksOpts, CtHook <- CtHooks],
    CtHooks1 = [
        {cth_tpx, #{role => top, ct_stdout_fingerprint => ProgressLineFingerprint, result_json => ResultOutput}},
        {cth_tpx, #{role => bot}}
        | CtHooks0
    ],
    CtHookHandle = {ct_hooks, CtHooks1},
    {CtHookHandle, CtOpts1}.

-doc """
Add a spec tuple to the list of ct_options if a tuple defining the property isn't present yet.
""".
-spec add_spec_if_absent({atom(), term()}, [term()]) -> [term()].
add_spec_if_absent({Key, Value}, CtOpts) ->
    case lists:keyfind(Key, 1, CtOpts) of
        false -> [{Key, Value} | CtOpts];
        _ -> CtOpts
    end.

-spec reorder_tests(list(#ct_test{}), #test_spec_test_case{}) -> list(#ct_test{}).
reorder_tests(Tests, #test_spec_test_case{testcases = TestCases}) ->
    % This is the ordered lists of test from the suite as
    % binary strings.
    MapNameToTests = lists:foldl(
        fun(#ct_test{canonical_name = Name} = Test, Map) -> Map#{unicode_characters_to_binary(Name) => Test} end,
        maps:new(),
        Tests
    ),
    lists:foldr(
        fun(#test_spec_test_info{name = TestName}, ListOrdered) ->
            case MapNameToTests of
                #{TestName := Test} -> [Test | ListOrdered];
                _ -> ListOrdered
            end
        end,
        [],
        TestCases
    ).

-doc """
LogDir is the directory where ct will log to.
Make sure it exists and returns it.
""".
-spec set_up_log_dir(file:filename_all()) -> file:filename_all().
set_up_log_dir(OutputDir) ->
    LogDir = filename:join(OutputDir, "log_dir"),
    ok = filelib:ensure_path(LogDir),
    LogDir.

-doc """
CtOpts must be tuple as defined here:
https://www.erlang.org/doc/apps/common_test/run_test_chapter.html#test-specification-syntax
that will be inserted to the test specification.
We do not check here that those are valid, but that they do not conflict with those
created here by the runner.
""".
-spec check_ct_opts([term()]) -> ok.
check_ct_opts(CtOpts) ->
    ProblematicsOpts = [suites, groups, cases, skip_suites, skip_groups, skip_cases],
    lists:foreach(
        fun(Opt) ->
            case lists:keyfind(Opt, 1, CtOpts) of
                false ->
                    ok;
                _ ->
                    ?LOG_ERROR("Option ~tp is not supported by test runner", [Opt]),
                    throw({non_valid_ct_opt, Opt})
            end
        end,
        ProblematicsOpts
    ).
