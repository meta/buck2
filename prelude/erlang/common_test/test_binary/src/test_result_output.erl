%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(test_result_output).
-moduledoc """
Write TPX standard test-result events as JSON Lines.

Common Test verdicts include group/suite teardown failures, so events are
published from the collected results once teardown has finished, rather than
from individual testcase callbacks. A suite watchdog timeout has no independent
testcase verdicts and is reported only as a run failure. Its timestamp is captured
when the runner observes the failure, before stopping test_exec or collecting logs.

A successful run with no collected cases emits an empty file, not a synthetic
passing testcase. Consumers remain responsible for checking expected test names.
""".
-compile(warn_missing_spec_all).

-export([write/5, output_file/1, status_name/1]).

-import(common_util, [unicode_characters_to_binary/1]).

-spec output_file(file:filename_all()) -> file:filename().
output_file(OutputDir) ->
    case os:getenv("TEST_RESULTS_OUTPUT_FILE") of
        false -> filename:join(OutputDir, "test_results.json");
        OutputFile -> OutputFile
    end.

-spec write(
    file:filename_all(),
    passed | failed | timeout,
    integer(),
    unicode:chardata(),
    [cth_tpx_test_tree:collected_result()]
) -> ok.
write(OutputDir, SuiteStatus, FailureTime, SuiteDetails, Results) ->
    OutputFile = output_file(OutputDir),
    {ok, File} = file:open(OutputFile, [write, raw, binary]),
    try
        Footer = footer(OutputDir),
        FooterIndex = local_footer_index(Results),
        IsSandcastle = os:getenv("SANDCASTLE") =/= false,
        lists:foreach(
            fun({Index, #{main := Main}}) ->
                write_case(
                    File,
                    Main,
                    case IsSandcastle orelse Index =:= FooterIndex of
                        true -> Footer;
                        false -> <<>>
                    end
                )
            end,
            lists:enumerate(Results)
        ),
        RunFooter =
            case {IsSandcastle, SuiteStatus, Results} of
                {false, failed, [_ | _]} -> <<>>;
                _ -> Footer
            end,
        write_run_failure(File, SuiteStatus, FailureTime, SuiteDetails, RunFooter)
    after
        ok = file:close(File)
    end.

%% TPX's print-passing-details option is not exposed to the test binary. Prefer
%% a failure so the footer is visible without it; otherwise keep it on the last case.
%% Zero denotes no cases, so no case can receive a footer.
-spec local_footer_index([cth_tpx_test_tree:collected_result()]) -> non_neg_integer().
local_footer_index(Results) ->
    lists:foldl(
        fun({Index, #{main := #{outcome := Outcome}}}, Last) ->
            case lists:member(Outcome, [failed, timeout, infra_failure]) of
                true -> Index;
                false -> Last
            end
        end,
        length(Results),
        lists:enumerate(Results)
    ).

-spec write_case(file:io_device(), cth_tpx_test_tree:collected_method_result(), binary()) -> ok.
write_case(File, #{name := Name, outcome := Outcome, details := Details, std_out := StdOut} = Result, Footer) when
    is_list(Name)
->
    %% All main-result producers append this suffix; reject inconsistent names
    %% rather than reporting results under a different testcase identity.
    [QualifiedNameChars, []] = string:split(Name, ".[main_testcase]", trailing),
    QualifiedName = unicode_characters_to_binary(QualifiedNameChars),
    {StartedTime, EndedTime, Duration} = timing(Result),
    write_event(File, #{start => #{name => QualifiedName, started_time => StartedTime}}),
    write_event(File, #{
        finish => #{
            name => QualifiedName,
            status => status(Outcome),
            ended_time => EndedTime,
            duration => Duration,
            message => unicode_characters_to_binary(["Details: ", Details, " \nStdOut: ", StdOut, " \n", Footer])
        }
    }).

-spec timing(cth_tpx_test_tree:collected_method_result()) -> {integer(), integer(), non_neg_integer()}.
timing(#{start_time := Start, end_time := End}) ->
    {round(Start * 1000), round(End * 1000), max(0, round((End - Start) * 1000))};
timing(_) ->
    {0, 0, 0}.

-spec status(cth_tpx_test_tree:outcome()) -> non_neg_integer().
status(passed) -> 0;
status(failed) -> 1;
status(timeout) -> 1;
status(skipped) -> 2;
status(omitted) -> 4;
status(infra_failure) -> 5.

-spec status_name(non_neg_integer()) -> passed | failed | skipped | omitted | infra_failure.
status_name(0) -> passed;
status_name(1) -> failed;
status_name(2) -> skipped;
status_name(4) -> omitted;
status_name(5) -> infra_failure.

-spec write_run_failure(file:io_device(), passed | failed | timeout, integer(), unicode:chardata(), binary()) -> ok.
write_run_failure(_File, passed, _Time, _Details, _Footer) ->
    ok;
write_run_failure(File, Status, Time, Details, Footer) ->
    FailureStatus =
        case Status of
            timeout -> 1;
            failed -> 2
        end,
    write_event(File, #{
        run_failure => #{
            status => FailureStatus,
            time => Time,
            details => unicode_characters_to_binary([Details, Footer])
        }
    }).

-spec write_event(file:io_device(), map()) -> ok.
write_event(File, Event) ->
    ok = file:write(File, [json:encode(Event), $\n]).

-spec footer(file:filename_all()) -> binary().
footer(OutputDir) ->
    case file:read_file(filename:join(OutputDir, "footer.txt")) of
        {ok, Footer} -> Footer;
        {error, enoent} -> <<>>
    end.
