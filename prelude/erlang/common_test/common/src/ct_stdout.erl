%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(ct_stdout).
-moduledoc """
Functions to work with the stdout of a Common Test, for example
to delimit the output of each test-case.
""".
-compile(warn_missing_spec_all).

-export([filename/0, filename/1]).
-export([make_fingerprint/0]).
-export([emit_progress/4]).
-export([process_raw_stdout_log/3, collect_method_stdout/4]).
-export([init_process_stdout_state/3, process_stdout_line/2]).
-export_type([progress/0, progress_line/0, offset/0, fingerprint/0, collected_stdout/0]).
-export_type([process_stdout_state/0]).

-import(common_util, [unicode_characters_to_binary/1]).

-define(UNICODE_REPLACEMENT_CHAR_UTF8, ~"�").

%% ---------------------------------------------------------------------------
%% Types
%% ---------------------------------------------------------------------------

-type progress() :: started | finished.
-type progress_line() :: binary().
-type offset() :: integer().

-doc """
A marker that is included on every progress line and can be used to distinguish such
markers among the rest of the stdout.
""".
-type fingerprint() :: binary().

-doc """
Mapping from a "start" marker to the stdout contents associated to that marker.
""".
-type collected_stdout() :: #{
    progress_line() => binary() | {truncated, binary()}
}.

-type process_stdout_state() :: #{
    fingerprint := fingerprint(),
    out_handle := file:fd(),
    write_to_stdout := boolean(),
    progress_markers_seen := #{progress_line() => offset()},
    has_pending_new_line := boolean(),
    pending_progress_marker := [binary()]
}.

%% ---------------------------------------------------------------------------
%% Public API
%% ---------------------------------------------------------------------------

-doc """
Filename of the CT stdout log file that will be returned to the user.
""".
-spec filename() -> file:filename_all().
filename() ->
    ~"ct_stdout.txt".

-doc """
Like `m:filename/0` but will prefix the given `OutputDir`.
""".
-spec filename(OutputDir) -> file:filename_all() when
    OutputDir :: file:filename_all().
filename(OutputDir) ->
    filename:join(OutputDir, filename()).

-spec make_fingerprint() -> fingerprint().
make_fingerprint() ->
    base64:encode(crypto:strong_rand_bytes(24)).

-doc """
Emit a unique line to stdout to mark the start/end of a ct_suite callback.

The logged line is returned so the relevant offset in the stdout file can later be found.

NB. `UserProcess` is expected to be the `user` process, but we demand an
explicit (cached) version, in case testcases mess with this process
""".
-spec emit_progress(UserProcess, Fingerprint, What, Progress) -> progress_line() when
    UserProcess :: pid(),
    Fingerprint :: fingerprint(),
    What :: unicode:chardata(),
    Progress :: progress().
emit_progress(UserProcess, Fingerprint, What, Progress) ->
    ProgressStr =
        case Progress of
            started -> ~"START";
            finished -> ~"END"
        end,
    Uniq = erlang:unique_integer([positive]),
    ProgressLine = unicode_characters_to_binary(
        io_lib:format(~"~ts[~ts] ~ts ~ts [~tp]", [Fingerprint, timestamp_str(), ProgressStr, What, Uniq])
    ),

    % NB. We emit a leading newling in case the last print statement
    % didn't finish the line. We'll manually remove it, if necessary when
    % processing the markers
    io:format(UserProcess, "~n~s~n", [ProgressLine]),

    ProgressLine.

-doc """
The returned value contains the collected stdout for each "start" marker, capped to
the last `MaxPerCollected` bytes.
""".
-spec collect_method_stdout(OutputFile, Offsets, TreeResults, MaxPerCollected) -> {ok, collected_stdout()} when
    OutputFile :: file:filename_all(),
    Offsets :: #{progress_line() => offset()},
    TreeResults :: cth_tpx_test_tree:tree(),
    MaxPerCollected :: non_neg_integer().
collect_method_stdout(OutputFile, Offsets, TreeResults, MaxPerCollected) ->
    MethodResults = get_method_results(TreeResults),
    {ok, InH} = file:open(OutputFile, [raw, binary, read, read_ahead]),
    try
        {ok,
            #{
                Start => collect_stdout(InH, StartOffset, EndOffset, MaxPerCollected)
             || #{start_progress_marker := Start, end_progress_marker := End} <- MethodResults,
                #{Start := StartOffset, End := EndOffset} <- [Offsets]
            }}
    after
        file:close(InH)
    end.

-doc """
Writes to `OutputFile` a copy of `RawStdoutFile` where all progress markers
have been removed.

The returned value contains the offset in `OutputFile` for each progress marker seen.
""".
-spec process_raw_stdout_log(Fingerprint, RawStdoutFile, OutputFile) -> {ok, #{progress_line() => offset()}} when
    Fingerprint :: fingerprint(),
    RawStdoutFile :: file:filename_all(),
    OutputFile :: file:filename_all().
process_raw_stdout_log(Fingerprint, RawStdoutFile, OutputFile) ->
    {ok, InHandle} = file:open(RawStdoutFile, [raw, binary, read, read_ahead]),

    State = init_process_stdout_state(Fingerprint, OutputFile, no_output_to_stdout),
    try
        {ok, process_raw_stdout_log_loop(InHandle, State)}
    after
        file:close(InHandle)
    end.

-spec init_process_stdout_state(Fingerprint, OutputFile, StdOutput) -> process_stdout_state() when
    Fingerprint :: fingerprint(),
    OutputFile :: file:filename_all(),
    StdOutput :: output_to_stdout | no_output_to_stdout.
init_process_stdout_state(Fingerprint, OutputFile, StdOutOutput) ->
    {ok, OutHandle} = file:open(OutputFile, [raw, write, delayed_write]),
    WriteToStdout =
        case StdOutOutput of
            output_to_stdout -> true;
            no_output_to_stdout -> false
        end,
    #{
        fingerprint => Fingerprint,
        out_handle => OutHandle,
        write_to_stdout => WriteToStdout,
        progress_markers_seen => #{},
        has_pending_new_line => false,
        pending_progress_marker => []
    }.

-spec process_stdout_line
    (eof, State) -> {eof, #{progress_line() => offset()}} when
        State :: process_stdout_state();
    ({eol | noeol, Line}, State0) -> {ok, State1} when
        Line :: binary(),
        State0 :: process_stdout_state(),
        State1 :: process_stdout_state().
process_stdout_line(eof, State0) ->
    State1 = flush_pending_newline(State0),
    #{out_handle := OutH, progress_markers_seen := Result} = State1,
    file:close(OutH),
    {eof, Result};
process_stdout_line({eol, Line}, State0) ->
    {ok, process_stdout_line_aux(eol, Line, State0)};
process_stdout_line({noeol, Line}, State0) ->
    {ok, process_stdout_line_aux(noeol, Line, State0)}.

%% ---------------------------------------------------------------------------
%% Helpers
%% ---------------------------------------------------------------------------

-spec timestamp_str() -> unicode:chardata().
timestamp_str() ->
    T = erlang:system_time(millisecond),
    calendar:system_time_to_rfc3339(T, [{unit, millisecond}, {time_designator, $\s}]).

-spec get_method_results(TreeResults) -> [cth_tpx_test_tree:method_result()] when
    TreeResults :: cth_tpx_test_tree:tree().
get_method_results(TreeResults) ->
    lists:flatten(get_method_results(TreeResults, [])).

-spec get_method_results(TreeResults, Acc) -> Acc when
    TreeResults :: cth_tpx_test_tree:tree() | [cth_tpx_test_tree:tree()],
    Acc :: [[cth_tpx_test_tree:method_result()]].
get_method_results([], Acc) ->
    Acc;
get_method_results([First | Rest], Acc0) ->
    Acc1 = get_method_results(First, Acc0),
    get_method_results(Rest, Acc1);
get_method_results(Leaf = #{type := leaf}, Acc) ->
    #{init_method := Init, main_method := Main, end_method := End} = Leaf,
    [filter_none([Init, Main, End]) | Acc];
get_method_results(Node = #{type := node}, Acc0) ->
    #{init_method := Init, end_method := End, test_cases := TestCases, sub_groups := SubGroups} = Node,
    Acc1 = [filter_none([Init, End]) | Acc0],
    Acc2 = get_method_results(maps:values(TestCases), Acc1),
    Acc3 = get_method_results(maps:values(SubGroups), Acc2),
    Acc3.

-spec filter_none([none | A]) -> [A].
filter_none([]) -> [];
filter_none([none | As]) -> filter_none(As);
filter_none([A | As]) when A /= none -> [A | filter_none(As)].

-spec process_raw_stdout_log_loop(InH, State) -> #{progress_line() => offset()} when
    InH :: file:fd(),
    State :: process_stdout_state().
process_raw_stdout_log_loop(InH, State0) ->
    case file:read_line(InH) of
        eof ->
            {eof, Result} = process_stdout_line(eof, State0),
            Result;
        {ok, Line} when is_binary(Line) ->
            <<Line1:(byte_size(Line) - 1)/binary, "\n">> = Line,
            {ok, State1} = process_stdout_line({eol, Line1}, State0),
            process_raw_stdout_log_loop(InH, State1)
    end.

-spec process_stdout_line_aux(Eol, Line, State0) -> State1 when
    Eol :: eol | noeol,
    Line :: binary(),
    State0 :: process_stdout_state(),
    State1 :: process_stdout_state().
process_stdout_line_aux(noeol, Line, State0 = #{pending_progress_marker := Chunks = [_ | _]}) ->
    % A progress marker split on several chunks, here's a new, non-terminal chunk
    State0#{pending_progress_marker := [Line | Chunks]};
process_stdout_line_aux(eol, Line, State0 = #{pending_progress_marker := Chunks = [_ | _]}) ->
    % A progress marker split on several chunks, here's the last chunk
    ProgressLine = unicode_characters_to_binary(lists:reverse([Line | Chunks])),
    State1 = State0#{pending_progress_marker => []},
    process_stdout_line_aux(eol, ProgressLine, State1);
process_stdout_line_aux(Eol, Line, State0) ->
    #{fingerprint := Fingerprint} = State0,
    case Line of
        <<Fingerprint:(byte_size(Fingerprint))/binary, _/binary>> when Eol =:= noeol ->
            % A progress marker split on several chunks, here's the first chunk
            State0#{pending_progress_marker => [Line]};
        <<Fingerprint:(byte_size(Fingerprint))/binary, _/binary>> ->
            #{out_handle := OutH, progress_markers_seen := Acc} = State0,
            {ok, Offset} = file:position(OutH, cur),
            Acc1 = Acc#{Line => Offset},
            State1 = State0#{progress_markers_seen => Acc1, has_pending_new_line => false},
            State1;
        ~"" when Eol =:= eol ->
            % This could be the leading newline of a marker, so postpone until we are sure. However
            State1 = flush_pending_newline(State0),
            State2 = State1#{has_pending_new_line => true},
            State2;
        _ when Eol =:= eol ->
            State1 = log_stdout([Line, ~"\n"], State0),
            State1;
        _ ->
            State1 = log_stdout(Line, State0),
            State1
    end.

-spec flush_pending_newline(State0) -> State1 when
    State0 :: process_stdout_state(),
    State1 :: process_stdout_state().
flush_pending_newline(State0) ->
    log_stdout([], State0).

-spec log_stdout(Chars, State0) -> State1 when
    Chars :: unicode:chardata(),
    State0 :: process_stdout_state(),
    State1 :: process_stdout_state().
log_stdout(Chars, State0) ->
    {Chars1, State1} =
        case State0 of
            #{has_pending_new_line := true} ->
                {[~"\n", Chars], State0#{has_pending_new_line => false}};
            _ ->
                {Chars, State0}
        end,
    case Chars1 of
        [] ->
            ok;
        ~"" ->
            ok;
        _ ->
            #{out_handle := OutH, write_to_stdout := WriteToStdout} = State0,
            file:write(OutH, Chars1),
            not WriteToStdout orelse io:put_chars(Chars1)
    end,
    State1.

-spec collect_stdout(InH, StartOffset, EndOffset, Max) -> binary() | {truncated, binary()} when
    InH :: file:fd(),
    StartOffset :: offset(),
    EndOffset :: offset(),
    Max :: non_neg_integer().
collect_stdout(_InH, StartOffset, EndOffset, _Max) when StartOffset =:= EndOffset ->
    ~"";
collect_stdout(InH, StartOffset, EndOffset, Max) when StartOffset < EndOffset ->
    Count = EndOffset - StartOffset,
    case Count =< Max of
        true ->
            case file:pread(InH, StartOffset, Count) of
                {ok, Result} when is_binary(Result) -> fix_unicode_encoding(Result)
            end;
        false ->
            First1 = EndOffset - Max,
            Count1 = Max,
            case file:pread(InH, First1, Count1) of
                {ok, Result} when is_binary(Result) ->
                    % Collected stdout is rendered and serialized as UTF-8.
                    % Sanitize before returning because truncation may start
                    % in the middle of a codepoint and tests may also emit
                    % arbitrary raw bytes.
                    {truncated, fix_unicode_encoding(Result)}
            end
    end.

-spec fix_unicode_encoding(binary()) -> binary().
fix_unicode_encoding(Bin) ->
    case unicode:characters_to_binary(Bin) of
        Ok when is_binary(Ok) -> Ok;
        {error, Prefix, Rest0} ->
            Rest = drop_invalid_utf8_prefix(require_binary(Rest0)),
            iolist_to_binary([Prefix, ?UNICODE_REPLACEMENT_CHAR_UTF8, fix_unicode_encoding(Rest)]);
        {incomplete, Prefix, _Rest} ->
            iolist_to_binary([Prefix, ?UNICODE_REPLACEMENT_CHAR_UTF8])
    end.

-spec require_binary(unicode:latin1_chardata() | unicode:chardata() | unicode:external_chardata()) -> binary().
require_binary(Bin) when is_binary(Bin) ->
    Bin.

-spec drop_invalid_utf8_prefix(binary()) -> binary().
drop_invalid_utf8_prefix(Bin) ->
    case unicode:characters_to_binary(Bin) of
        {error, ~"", <<_:8, Rest/binary>>} ->
            drop_invalid_utf8_prefix(Rest);
        {incomplete, ~"", _Rest} ->
            ~"";
        _ ->
            Bin
    end.
