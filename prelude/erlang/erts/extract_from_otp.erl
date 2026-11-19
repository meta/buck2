%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
-module(extract_from_otp).
-author("loscher@meta.com").
-moduledoc """
Take parts of the OTP a release is built from, and introspect the versions it ships

usage:
  extract_from_otp.erl versions Target
  extract_from_otp.erl extract_into Wildcard Target
  extract_from_otp.erl Wildcard Target
""".
-compile(warn_missing_spec_all).
-compile({inline, [bin/1]}).

%% escript API
-export([
    main/1
]).

%% Macros
-define(otp_application_not_found, 1).
-define(no_matches_for_wildcard, 2).
-define(multiple_matches_for_wildcard, 3).
-define(extract_from_otp_error(ErrorNumber, Report),
    io:format(standard_error, "~ts:~w:~w: ~ts\n", [?FILE, ?LINE, ErrorNumber, json:encode(Report)])
).
-define(FMT(Str, Args), bin(io_lib:format(Str, Args))).

%%%=============================================================================
%%% escript API functions
%%%=============================================================================

-spec main([string()]) -> ok.
main(["versions", Target]) ->
    ok = file:write_file(Target, json:encode(versions()));
main(["extract_into", Wildcard, Target]) ->
    ok = extract_into(Wildcard, Target);
main([Wildcard, Target]) ->
    ok = extract(Wildcard, Target);
main(_) ->
    usage().

%%%-----------------------------------------------------------------------------
%%% Internal functions
%%%-----------------------------------------------------------------------------

-spec bin(IoListOrBinary :: iolist() | binary()) -> binary().
bin(X) -> erlang:iolist_to_binary(X).

-spec copy_dir(From, To) -> ok when From :: file:filename(), To :: file:filename().
copy_dir(From, To) ->
    Cmd = lists:flatten(
        io_lib:format("cp -r ~ts ~ts", [From, To])
    ),
    io:format(standard_error, "cmd: ~ts~n~ts~n~n", [Cmd, os:cmd(Cmd)]),
    case filelib:is_dir(To, prim_file) orelse filelib:is_file(To, prim_file) of
        true -> ok;
        false -> erlang:halt(1)
    end.

-spec extract(string(), string()) -> ok.
extract(Wildcard, Target) ->
    FullWildcard = filename:join(code:root_dir(), Wildcard),
    case filelib:wildcard(FullWildcard, ".", prim_file) of
        [OneResult] ->
            ok = copy_dir(OneResult, Target);
        [] ->
            % Extract app name from wildcard for structured error reporting
            case extract_app_name_from_wildcard(Wildcard) of
                {ok, AppName} ->
                    AvailableApps = get_available_otp_applications(),
                    ?extract_from_otp_error(?otp_application_not_found, #{
                        message => ?FMT("OTP application not found: ~ts", [AppName]),
                        name => bin(AppName),
                        available_applications => AvailableApps,
                        root_dir => bin(code:root_dir())
                    });
                error ->
                    ?extract_from_otp_error(?no_matches_for_wildcard, #{
                        message => ?FMT("No matches found for wildcard: ~ts", [FullWildcard]),
                        wildcard => bin(FullWildcard),
                        root_dir => bin(code:root_dir())
                    })
            end,
            erlang:halt(1);
        Paths ->
            ?extract_from_otp_error(?multiple_matches_for_wildcard, #{
                message => ?FMT("Expected exactly one result but found ~p matches for: ~ts", [
                    length(Paths), FullWildcard
                ]),
                paths => [bin(Path) || Path <- Paths],
                wildcard => bin(FullWildcard),
                root_dir => bin(code:root_dir())
            }),
            erlang:halt(1)
    end.

-spec extract_into(Wildcard, Target) -> ok when Wildcard :: string(), Target :: string().
extract_into(Wildcard, Target) ->
    FullWildcard = filename:join(code:root_dir(), Wildcard),
    case filelib:wildcard(FullWildcard, ".", prim_file) of
        [] ->
            ?extract_from_otp_error(?no_matches_for_wildcard, #{
                message => ?FMT("No matches found for wildcard: ~ts", [FullWildcard]),
                wildcard => bin(FullWildcard),
                root_dir => bin(code:root_dir())
            }),
            erlang:halt(1);
        Paths ->
            ok = filelib:ensure_path(Target),
            lists:foreach(
                fun(Path) ->
                    ok = copy_dir(Path, filename:join(Target, filename:basename(Path)))
                end,
                Paths
            )
    end.

-spec versions() -> #{binary() => binary() | #{binary() => binary()}}.
versions() ->
    LibDir = filename:join(code:root_dir(), "lib"),
    Applications = lists:foldl(
        fun(Entry, Acc) ->
            case string:split(Entry, "-", trailing) of
                [Name, Version] -> Acc#{bin(Name) => bin(Version)};
                _ -> Acc
            end
        end,
        #{},
        filelib:wildcard("*-*", LibDir, prim_file)
    ),
    #{
        <<"erts_version">> => bin(erlang:system_info(version)),
        <<"applications">> => Applications
    }.

-spec extract_app_name_from_wildcard(string()) -> {ok, string()} | error.
extract_app_name_from_wildcard(Wildcard) ->
    % Expected format: "lib/appname-*"
    case string:split(Wildcard, "/") of
        ["lib", Pattern] ->
            case string:split(Pattern, "-") of
                [AppName, "*"] -> {ok, AppName};
                _ -> error
            end;
        _ ->
            error
    end.

-spec get_available_otp_applications() -> [binary()].
get_available_otp_applications() ->
    RootDir = code:root_dir(),
    % Read OTP version from releases/start_erl.data (format: "ERTS_VERSION OTP_VERSION")
    StartErlDataFile = filename:join([RootDir, "releases", "start_erl.data"]),
    maybe
        {ok, StartErlDataContent} ?= file:read_file(StartErlDataFile),
        % Parse "15.1 27\n" to get OTP version "27"
        [_ErtsVersion, OtpVersion] ?= string:split(string:trim(StartErlDataContent), " "),
        % Read installed_application_versions file
        InstalledAppsFile = filename:join([RootDir, "releases", OtpVersion, "installed_application_versions"]),
        {ok, InstalledAppsContent} ?= file:read_file(InstalledAppsFile),
        % Parse lines like "stdlib-6.1" to extract app names
        InstalledAppsLines = [_ | _] ?= string:split(string:trim(InstalledAppsContent), "\n", all),
        parse_installed_application_versions(InstalledAppsLines)
    else
        _ ->
            % Fallback to lib directory wildcard if:
            %   start_erl.data doesn't exist,
            %   start_erl.data format is unexpected,
            %   installed_application_versions doesn't exist,
            %   or installed_application_versions format is unexpected.
            get_available_otp_applications_from_lib()
    end.

-spec get_available_otp_applications_from_lib() -> [binary()].
get_available_otp_applications_from_lib() ->
    % Fallback method: scan lib directory
    LibDir = filename:join(code:root_dir(), "lib"),
    case filelib:wildcard("*", LibDir, prim_file) of
        [] -> [];
        Dirs -> parse_installed_application_versions(Dirs)
    end.

-spec parse_installed_application_versions(Dirs) -> AppNameList when Dirs :: [string()], AppNameList :: [binary()].
parse_installed_application_versions(Dirs) ->
    % Extract app names from directory names (format: appname-version)
    % Sort alphabetically for consistent output
    lists:sort(
        lists:filtermap(
            fun(Dir) ->
                case string:split(Dir, "-") of
                    [AppName | _] -> {true, bin(AppName)};
                    _ -> false
                end
            end,
            Dirs
        )
    ).

-spec usage() -> ok.
usage() ->
    io:format(
        standard_error,
        "usage:~n"
        "  extract_from_otp.erl versions Target~n"
        "  extract_from_otp.erl extract_into Wildcard Target~n"
        "  extract_from_otp.erl Wildcard Target~n",
        []
    ).
