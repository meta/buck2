/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

'use strict';

// Runs the test files of a `nodejs_test` for the `js_test` external runner.
// Without `--write-test-results`, a `TEST_RESULTS_OUTPUT_FILE` inherited from
// an enclosing test is left alone.
//
// Loaded by `--test-reporter`, this file is instead the node:test reporter.

const childProcess = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {StringDecoder} = require('node:string_decoder');
const {pathToFileURL} = require('node:url');
const util = require('node:util');

const SEPARATOR = ' > ';
const PASS = 0;
const FAIL = 1;
const SKIP = 2;
const RUN_FAILURE_FATAL = 2;
// How much output a failed result keeps; all of it is still printed.
const MAX_OUTPUT_BYTES = 64 * 1024;
const JEST_RESERVED_FLAGS = [
  '--changedFilesWithAncestor',
  '--changed-files-with-ancestor',
  '--changedSince',
  '--changed-since',
  '--ci',
  '--collectCoverage',
  '--collect-coverage',
  '--config',
  '--coverage',
  '--coverageDirectory',
  '--coverage-directory',
  '--coverageReporters',
  '--coverage-reporters',
  '--findRelatedTests',
  '--find-related-tests',
  '--json',
  '--lastCommit',
  '--last-commit',
  '--outputFile',
  '--output-file',
  '--listTests',
  '--list-tests',
  '--onlyChanged',
  '--only-changed',
  '--onlyFailures',
  '--only-failures',
  '--runTestsByPath',
  '--run-tests-by-path',
  '--shard',
  '--testNamePattern',
  '--test-name-pattern',
  '--watch',
  '--watchAll',
  '--watch-all',
  '--watchman',
];
const JEST_RESERVED_SHORT_FLAGS = 'cfot';
const VITEST_RESERVED_FLAGS = [
  '--changed',
  '--config',
  '--coverage',
  '--dir',
  '--exclude',
  '--filesOnly',
  '--files-only',
  '--json',
  '--mergeReports',
  '--merge-reports',
  '--outputFile',
  '--output-file',
  '--related',
  '--reporter',
  '--root',
  '--run',
  '--shard',
  '--testNamePattern',
  '--test-name-pattern',
  '--watch',
];
const VITEST_RESERVED_SHORT_FLAGS = 'crtw';

// Splits a group of short flags the way Jest's parser, yargs-parser, does: each
// character is a flag until one is followed by `=`, a non-word character or a
// number, which is its value.
function hasReservedShortFlag(arg) {
  for (let j = 1; j < arg.length; j++) {
    if (JEST_RESERVED_SHORT_FLAGS.includes(arg[j])) {
      return true;
    }
    const rest = arg.slice(j + 1);
    if (/^\W/.test(rest) || /^-?\d+(\.\d*)?(e-?\d+)?$/.test(rest)) {
      return false;
    }
  }
  return false;
}

function isReservedJestArg(arg) {
  if (!arg.startsWith('--')) {
    return arg.startsWith('-') && hasReservedShortFlag(arg);
  }
  const flag = arg.split('=')[0].replace(/^--no-/, '--');
  return JEST_RESERVED_FLAGS.includes(flag);
}

function hasReservedVitestShortFlag(arg) {
  for (let j = 1; j < arg.length; j++) {
    if (VITEST_RESERVED_SHORT_FLAGS.includes(arg[j])) {
      return true;
    }
    const rest = arg.slice(j + 1);
    if (/^\W/.test(rest) || /^-?\d+(\.\d*)?(e-?\d+)?$/.test(rest)) {
      return false;
    }
  }
  return false;
}

function isReservedVitestArg(arg) {
  if (!arg.startsWith('--')) {
    return arg.startsWith('-') && hasReservedVitestShortFlag(arg);
  }
  const flag = arg.split('=')[0].replace(/^--no-/, '--');
  return (
    VITEST_RESERVED_FLAGS.includes(flag) || flag.startsWith('--coverage.')
  );
}

// Reported by its message alone, without a stack.
class AdapterError extends Error {}

function parseCoveragePaths(text) {
  let paths;
  try {
    paths = JSON.parse(text);
  } catch {
    paths = undefined;
  }
  if (!Array.isArray(paths) || !paths.every(p => typeof p === 'string')) {
    throw new AdapterError('--coverage-paths must be a JSON array of strings');
  }
  return paths;
}

function parseArgs(argv) {
  const options = {
    framework: undefined,
    files: [],
    filters: [],
    list: false,
    output: undefined,
    nodeArgs: [],
    jest: undefined,
    vitest: undefined,
    coveragePrefix: undefined,
    coveragePaths: undefined,
    config: undefined,
    root: undefined,
    args: [],
    writeTestResults: false,
  };
  let i = 0;
  const value = flag => {
    if (i >= argv.length) {
      throw new AdapterError(`${flag} requires a value`);
    }
    return argv[i++];
  };
  while (i < argv.length) {
    const arg = argv[i++];
    switch (arg) {
      case '--framework':
        options.framework = value(arg);
        break;
      case '--test-file':
        options.files.push({name: value(arg), path: value(arg)});
        break;
      case '--filter':
        options.filters.push(value(arg));
        break;
      case '--list':
        options.list = true;
        break;
      case '--output':
        options.output = value(arg);
        break;
      case '--node-arg':
        options.nodeArgs.push(value(arg));
        break;
      case '--jest':
        options.jest = value(arg);
        break;
      case '--vitest':
        options.vitest = value(arg);
        break;
      case '--coverage-prefix':
        options.coveragePrefix = value(arg);
        break;
      case '--coverage-paths':
        options.coveragePaths = parseCoveragePaths(value(arg));
        break;
      case '--config':
        options.config = value(arg);
        break;
      case '--root':
        options.root = value(arg);
        break;
      case '--arg':
        options.args.push(value(arg));
        break;
      case '--write-test-results':
        options.writeTestResults = true;
        break;
      default:
        throw new AdapterError(`unexpected argument '${arg}'`);
    }
  }
  if (options.framework === 'jest') {
    if (options.jest === undefined || options.root === undefined) {
      throw new AdapterError('--framework jest requires --jest and --root');
    }
    const reserved = options.args.find(isReservedJestArg);
    if (reserved !== undefined) {
      throw new AdapterError(
        `Jest argument '${reserved}' is set by the test adapter or changes which test files run, and cannot be passed in \`args\``,
      );
    }
  } else if (options.framework === 'vitest') {
    if (options.vitest === undefined || options.root === undefined) {
      throw new AdapterError('--framework vitest requires --vitest and --root');
    }
    const reserved = options.args.find(isReservedVitestArg);
    if (reserved !== undefined) {
      throw new AdapterError(
        `Vitest argument '${reserved}' is set by the test adapter or changes which test files run, and cannot be passed in \`args\``,
      );
    }
  } else if (options.framework !== 'node') {
    throw new AdapterError(`unsupported --framework '${options.framework}'`);
  }
  if (options.framework === 'node') {
    if (options.coveragePrefix !== undefined) {
      throw new AdapterError('--coverage-prefix requires --framework jest or vitest');
    }
    if (options.coveragePaths !== undefined) {
      throw new AdapterError('--coverage-paths requires --framework jest or vitest');
    }
  }
  return options;
}

function writeListing(files, output) {
  const listing = files
    .map(file => JSON.stringify({testcase: file.name}) + '\n')
    .join('');
  if (output === undefined) {
    process.stdout.write(listing);
  } else {
    fs.writeFileSync(output, listing);
  }
}

function escapeRegExp(text) {
  return text.replace(/[\\^$.*+?()[\]{}|]/g, '\\$&');
}

// A run is `whole` when there are no filters or a filter names its file; its
// `filters` name tests or `describe` blocks in it.
function selectRuns(files, filters) {
  if (filters.length === 0) {
    return files.map(file => ({file, whole: true, filters: []}));
  }
  const runsByFile = new Map();
  const runOf = file => {
    let run = runsByFile.get(file);
    if (run === undefined) {
      run = {file, whole: false, filters: new Set()};
      runsByFile.set(file, run);
    }
    return run;
  };
  for (const filter of filters) {
    const whole = files.find(file => file.name === filter);
    if (whole !== undefined) {
      runOf(whole).whole = true;
      continue;
    }
    let file;
    for (const candidate of files) {
      if (
        filter.startsWith(candidate.name + SEPARATOR) &&
        (file === undefined || candidate.name.length > file.name.length)
      ) {
        file = candidate;
      }
    }
    if (file === undefined) {
      throw new AdapterError(
        `unknown filter '${filter}': expected a test file name or '<test file name>${SEPARATOR}<test name>'`,
      );
    }
    runOf(file).filters.add(filter);
  }
  return files
    .filter(file => runsByFile.has(file))
    .map(file => {
      const run = runsByFile.get(file);
      return {file, whole: run.whole, filters: [...run.filters]};
    });
}

// node:test and Jest match a name pattern against a test's name prefixed with
// its ancestors' names, separated by spaces. A separator in `filter` can also be
// part of a name, so the pattern accepts either.
function fullNamePattern(file, filter) {
  return filter
    .slice(file.name.length + SEPARATOR.length)
    .split(SEPARATOR)
    .map(escapeRegExp)
    .join(`(?:${SEPARATOR}| )`);
}

// Jest matches only tests, so `descendants` also accepts the tests inside the
// describe block `filter` names.
function namePattern(file, filter, descendants = false) {
  return `^${fullNamePattern(file, filter)}${descendants ? '(?: .*)?' : ''}$`;
}

function resultNameRegExp(file, filter, flags) {
  return new RegExp(
    `^${escapeRegExp(file.name + SEPARATOR)}${fullNamePattern(file, filter)}(?:${SEPARATOR}|$)`,
    flags,
  );
}

function printNote(message) {
  process.stderr.write(`nodejs_test: ${message}\n`);
}

function filterResults(
  file,
  tests,
  suites,
  {whole, filters, ended, endedTime, flags = ''},
) {
  if (filters.length === 0) {
    return tests;
  }
  const patterns = new Map(
    filters.map(filter => [filter, resultNameRegExp(file, filter, flags)]),
  );
  const selects = (filter, name) => patterns.get(filter).test(name);
  const selected = whole
    ? tests
    : tests.filter(test => filters.some(filter => selects(filter, test.name)));
  if (!ended) {
    return selected;
  }
  const unmatched = filters
    .filter(
      filter =>
        !tests.some(test => selects(filter, test.name)) &&
        !suites.some(suite => selects(filter, suite)),
    )
    .map(filter => {
      printNote(`no test matched the filter '${filter}'`);
      return {
        name: filter,
        status: FAIL,
        message: 'no test matched this filter',
        duration: 0,
        endedTime,
      };
    });
  return [...selected, ...unmatched];
}

const children = new Set();

// Re-raising the signal ends this process without running `finally` blocks,
// so the handler removes the work directory itself.
function killChildrenOnSignal(getWorkDir) {
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.once(signal, () => {
      for (const child of children) {
        child.kill(signal);
      }
      const workDir = getWorkDir();
      if (workDir !== undefined) {
        fs.rmSync(workDir, {recursive: true, force: true});
      }
      process.kill(process.pid, signal);
    });
  }
}

function linePrefixer(stream, prefix) {
  const decoder = new StringDecoder('utf8');
  let partial = '';
  const flush = text => {
    const lines = (partial + text).split('\n');
    partial = lines.pop();
    if (lines.length > 0) {
      stream.write(lines.map(line => `${prefix}${line}\n`).join(''));
    }
  };
  return {
    write: chunk => flush(decoder.write(chunk)),
    end: () => {
      flush(decoder.end());
      if (partial !== '') {
        stream.write(`${prefix}${partial}\n`);
        partial = '';
      }
    },
  };
}

function spawnNode(nodeArgs, args, {prefix, onStdout} = {}) {
  const started = Date.now();
  return new Promise(resolve => {
    const output = [];
    let outputBytes = 0;
    const keep = chunk => {
      output.push(chunk);
      outputBytes += chunk.length;
      while (outputBytes - output[0].length >= MAX_OUTPUT_BYTES) {
        outputBytes -= output.shift().length;
      }
    };
    const passThrough = stream =>
      prefix === undefined
        ? {write: chunk => stream.write(chunk), end: () => {}}
        : linePrefixer(stream, prefix);
    const stdout =
      onStdout === undefined
        ? passThrough(process.stdout)
        : {write: onStdout, end: () => {}};
    const stderr = passThrough(process.stderr);
    let settled = false;
    const outcome = fields => {
      if (settled) {
        return;
      }
      settled = true;
      stdout.end();
      stderr.end();
      resolve({
        code: null,
        signal: null,
        error: undefined,
        durationMs: Date.now() - started,
        output: Buffer.concat(output),
        ...fields,
      });
    };
    const child = childProcess.spawn(
      process.execPath,
      [...process.execArgv, ...nodeArgs, ...args],
      {stdio: ['ignore', 'pipe', 'pipe']},
    );
    children.add(child);
    child.stdout.on('data', chunk => {
      stdout.write(chunk);
      keep(chunk);
    });
    child.stderr.on('data', chunk => {
      stderr.write(chunk);
      keep(chunk);
    });
    child.on('error', error => {
      children.delete(child);
      outcome({error});
    });
    child.on('close', (code, signal) => {
      children.delete(child);
      outcome({code, signal});
    });
  });
}

function processDetails(command, outcome, describeExit, notes = []) {
  const lines = [...notes];
  if (outcome.error !== undefined) {
    lines.push(`could not run ${command}: ${outcome.error.message}`);
  } else if (outcome.signal !== null) {
    lines.push(`${command} was killed by ${outcome.signal}`);
  } else if (describeExit) {
    lines.push(`${command} exited with code ${outcome.code}`);
  }
  const {output} = outcome;
  if (output.length > 0) {
    let start = Math.max(0, output.length - MAX_OUTPUT_BYTES);
    // Skip UTF-8 continuation bytes.
    while (start < output.length && (output[start] & 0xc0) === 0x80) {
      start++;
    }
    const kept = output.subarray(start);
    lines.push(...(lines.length > 0 ? ['', kept.toString()] : [kept.toString()]));
  }
  return lines.join('\n');
}

function testFileResult(file, tests, details, duration, endedTime) {
  const failed = tests.filter(test => test.status === FAIL);
  const result = {name: file.name, duration, endedTime};
  if (details !== undefined || failed.length > 0) {
    const lines = failed.map(test => `failed: ${test.name}`);
    if (details !== undefined) {
      lines.push(...(lines.length > 0 ? ['', details] : [details]));
    }
    result.status = FAIL;
    result.message = lines.join('\n');
  } else if (tests.length > 0 && tests.every(test => test.status === SKIP)) {
    result.status = SKIP;
  } else {
    result.status = PASS;
  }
  return result;
}

function readEvents(eventsFile) {
  let text;
  try {
    text = fs.readFileSync(eventsFile, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') {
      return {events: [], malformed: 0};
    }
    throw error;
  }
  // A crash can cut short the last event, which has no newline after it.
  const lines = text.split('\n');
  const events = [];
  let malformed = 0;
  for (const [index, line] of lines.entries()) {
    if (line === '') {
      continue;
    }
    try {
      events.push(JSON.parse(line));
    } catch {
      if (index !== lines.length - 1) {
        malformed++;
      }
    }
  }
  return {events, malformed};
}

// `unfinished` lists the innermost tests that started running but never
// reported, if the run did not end.
function nodeTestReport(file, events, endedTime) {
  const tests = [];
  const suites = [];
  let ended = false;
  const reporting = [];
  const running = [];
  const unfinished = [];
  const fullName = (ancestors, event) =>
    [file.name, ...ancestors.slice(0, event.nesting), event.name].join(
      SEPARATOR,
    );
  const location = event =>
    [event.file, event.line, event.column, event.nesting, event.name].join(
      ':',
    );
  for (const event of events) {
    if (event.type === 'test:plan') {
      ended = true;
      continue;
    }
    if (event.type === 'test:dequeue') {
      // Tests with `concurrency` interleave, so these ancestors are a guess.
      running.length = event.nesting;
      running.push(event.name);
      unfinished.push({
        name: fullName(running, event),
        location: location(event),
      });
      continue;
    }
    if (event.type === 'test:start') {
      reporting.length = event.nesting;
      reporting.push(event.name);
      continue;
    }
    const name = fullName(reporting, event);
    const eventLocation = location(event);
    const index = unfinished.findLastIndex(
      entry => entry.location === eventLocation,
    );
    if (index !== -1) {
      unfinished.splice(index, 1);
    }
    // A suite or a test with subtests fails when a subtest does, and that
    // subtest has its own result. A suite that passes has results for its
    // tests, unless it was skipped and they never ran.
    if (event.failureType === 'subtestsFailed') {
      continue;
    }
    if (event.suite && event.type === 'test:pass' && !event.skip && !event.todo) {
      suites.push(name);
      continue;
    }
    const result = {
      name,
      duration: Math.round(event.durationMs ?? 0),
      endedTime: event.endedTime ?? endedTime,
    };
    // `skip` and `todo` are `true` or the reason.
    const skipped = event.skip || event.todo;
    if (skipped) {
      result.status = SKIP;
      if (typeof skipped === 'string') {
        result.message = skipped;
      }
    } else if (event.type === 'test:fail') {
      result.status = FAIL;
      result.message = event.error;
    } else {
      result.status = PASS;
    }
    tests.push(result);
  }
  return {
    tests,
    suites,
    ended,
    unfinished: ended
      ? []
      : unfinished
          .map(entry => entry.name)
          .filter(
            (name, _, names) =>
              !names.some(other => other.startsWith(name + SEPARATOR)),
          ),
  };
}

// Runs `node <file>`, not `node --test`, whose positional arguments are globs:
// a path containing `[...]`, `{...}` or `@(...)` would run nothing and pass.
async function runNodeTestFile(options, run, eventsFile, prefix) {
  const outcome = await spawnNode(options.nodeArgs, [
    `--test-reporter=${pathToFileURL(__filename).href}`,
    `--test-reporter-destination=${eventsFile}`,
    '--test-reporter=spec',
    '--test-reporter-destination=stdout',
    ...(run.whole ? [] : run.filters).map(
      filter => `--test-name-pattern=${namePattern(run.file, filter)}`,
    ),
    run.file.path,
  ], {prefix});
  const endedTime = Date.now();
  let events = [];
  let malformed = 0;
  let readError;
  try {
    ({events, malformed} = readEvents(eventsFile));
  } catch (error) {
    readError = error;
  }
  const report = nodeTestReport(run.file, events, endedTime);
  const tests = filterResults(run.file, report.tests, report.suites, {
    whole: run.whole,
    filters: run.filters,
    ended: report.ended,
    endedTime,
  });
  const failed = tests.some(test => test.status === FAIL);
  const crashed = outcome.error !== undefined || outcome.code !== 0;
  const notes = report.unfinished.map(name => `did not finish: ${name}`);
  if (readError !== undefined) {
    notes.push(`could not read the node:test events: ${readError.message}`);
  } else if (!report.ended && !crashed) {
    notes.push(
      'node:test did not report the end of the run: the file defines no tests, or it exited before they finished (by calling `process.exit()`, for example)',
    );
  }
  if (malformed > 0) {
    notes.push(`${malformed} node:test event(s) were malformed and dropped`);
  }
  for (const note of notes) {
    printNote(`${run.file.name}: ${note}`);
  }
  const details =
    failed || crashed || notes.length > 0
      ? processDetails('node', outcome, !failed, notes)
      : undefined;
  return [
    ...tests,
    testFileResult(run.file, tests, details, outcome.durationMs, endedTime),
  ];
}

async function runNodeTests(options, runs, workDir) {
  // Before Node 22, a name pattern matches a test's own name only, so it
  // cannot name a test inside a describe block.
  const [major] = process.versions.node.split('.').map(Number);
  const unsupported = run => major < 22 && !run.whole && run.filters.length > 0;
  // The default concurrency of `node --test`.
  const parallelism = Math.min(
    runs.length,
    Math.max(1, (os.availableParallelism?.() ?? os.cpus().length) - 1),
  );
  const resultsByRun = [];
  let next = 0;
  const worker = async () => {
    while (next < runs.length) {
      const index = next++;
      const run = runs[index];
      if (unsupported(run)) {
        const details = `filtering by test or describe block (${run.filters.join(', ')}) needs Node 22 or later, and this is Node ${process.versions.node}; filter by test file instead`;
        printNote(`${run.file.name}: ${details}`);
        resultsByRun[index] = [
          testFileResult(run.file, [], details, 0, Date.now()),
        ];
        continue;
      }
      try {
        resultsByRun[index] = await runNodeTestFile(
          options,
          run,
          path.join(workDir, `${index}.jsonl`),
          parallelism > 1 ? `[${run.file.name}] ` : undefined,
        );
      } catch (error) {
        const details = `the adapter failed while running this file: ${error?.stack ?? error}`;
        printNote(`${run.file.name}: ${details}`);
        resultsByRun[index] = [
          testFileResult(run.file, [], details, 0, Date.now()),
        ];
      }
    }
  };
  await Promise.all(Array.from({length: parallelism}, worker));
  return {results: resultsByRun.flat(), runFailures: []};
}

function jestArgs(options) {
  return [
    path.join(options.jest, 'jest', 'bin', 'jest.js'),
    '--ci',
    '--watchman=false',
    '--config',
    options.config ?? JSON.stringify({rootDir: options.root}),
    ...options.args,
  ];
}

function pathKeys(file) {
  const keys = [path.resolve(file)];
  try {
    keys.push(fs.realpathSync(file));
  } catch {
    // A path that does not exist has no real path.
  }
  return keys;
}

function fileFinder(files) {
  const filesByKey = new Map();
  for (const file of files) {
    for (const key of pathKeys(file.path)) {
      if (!filesByKey.has(key)) {
        filesByKey.set(key, file);
      }
    }
  }
  return reportedPath =>
    pathKeys(reportedPath)
      .map(key => filesByKey.get(key))
      .find(file => file !== undefined);
}

async function jestTestFiles(options) {
  const stdout = [];
  const outcome = await spawnNode(
    options.nodeArgs,
    [
      ...jestArgs(options),
      '--listTests',
      '--json',
      '--runTestsByPath',
      ...options.files.map(file => file.path),
    ],
    {
      onStdout: chunk => {
        stdout.push(chunk);
        process.stderr.write(chunk);
      },
    },
  );
  if (outcome.error !== undefined || outcome.code !== 0) {
    throw new AdapterError(processDetails('jest --listTests', outcome, true));
  }
  const listed = Buffer.concat(stdout)
    .toString()
    .split('\n')
    .reverse()
    .map(line => {
      try {
        return JSON.parse(line);
      } catch {
        return undefined;
      }
    })
    .find(Array.isArray);
  if (listed === undefined) {
    throw new AdapterError(
      `jest --listTests printed no test list\n${processDetails('jest --listTests', outcome, false)}`,
    );
  }
  const findFile = fileFinder(options.files);
  const selected = new Set(listed.map(findFile));
  const files = options.files.filter(file => selected.has(file));
  if (files.length === 0) {
    throw new AdapterError(
      `Jest selects none of ${options.files.map(file => `'${file.name}'`).join(', ')}: check \`testMatch\`, \`testRegex\` and \`testPathIgnorePatterns\` in the Jest config`,
    );
  }
  return files;
}

function isStringArray(value) {
  return Array.isArray(value) && value.every(item => typeof item === 'string');
}

function isJestSuite(suite) {
  return (
    typeof suite?.name === 'string' &&
    typeof suite.status === 'string' &&
    typeof suite.startTime === 'number' &&
    typeof suite.endTime === 'number' &&
    typeof suite.message === 'string' &&
    Array.isArray(suite.assertionResults) &&
    suite.assertionResults.every(
      test =>
        typeof test?.title === 'string' &&
        typeof test.status === 'string' &&
        isStringArray(test.ancestorTitles) &&
        isStringArray(test.failureMessages) &&
        (test.duration == null || typeof test.duration === 'number'),
    )
  );
}

function readJestReport(reportFile) {
  const report = JSON.parse(fs.readFileSync(reportFile, 'utf8'));
  if (
    typeof report?.success !== 'boolean' ||
    !Array.isArray(report.testResults)
  ) {
    throw new Error('it is not a Jest `--json` report');
  }
  return report;
}

function jestTestResults(file, suite, endedTime) {
  return suite.assertionResults.map(test => {
    const result = {
      name: [file.name, ...test.ancestorTitles, test.title].join(SEPARATOR),
      duration: Math.round(test.duration ?? 0),
      endedTime,
    };
    if (test.status === 'passed') {
      result.status = PASS;
    } else if (test.status === 'failed') {
      result.status = FAIL;
      result.message = test.failureMessages.join('\n');
    } else {
      result.status = SKIP;
    }
    return result;
  });
}

async function runJestGroup(options, group, reportFile, coverageDirectory) {
  const files = group.runs.map(run => run.file);
  const patterns = group.runs.flatMap(run =>
    run.whole ? [] : run.filters.map(filter => namePattern(run.file, filter, true)),
  );
  const outcome = await spawnNode(options.nodeArgs, [
    ...jestArgs(options),
    '--json',
    `--outputFile=${reportFile}`,
    ...(patterns.length > 0 ? ['--testNamePattern', patterns.join('|')] : []),
    ...(coverageDirectory === undefined
      ? []
      : [
          '--coverage',
          '--coverageReporters=json',
          `--coverageDirectory=${coverageDirectory}`,
        ]),
    '--runTestsByPath',
    ...files.map(file => file.path),
  ]);
  const endedTime = Date.now();
  let report;
  let readError;
  try {
    report = readJestReport(reportFile);
  } catch (error) {
    readError = error;
  }
  const findFile = fileFinder(files);
  const results = [];
  const suites = new Map();
  const unselectedFailed = [];
  let malformed = 0;
  for (const suite of report?.testResults ?? []) {
    if (!isJestSuite(suite)) {
      malformed++;
      continue;
    }
    const file = findFile(suite.name);
    if (file === undefined) {
      if (suite.status === 'failed') {
        unselectedFailed.push(suite.name);
      }
      continue;
    }
    // A file in several Jest projects is reported once for each.
    const merged = suites.get(file);
    suites.set(
      file,
      merged === undefined
        ? suite
        : {
            status:
              merged.status === 'failed' || suite.status === 'failed'
                ? 'failed'
                : suite.status,
            message: [merged.message, suite.message]
              .filter(message => message !== '')
              .join('\n'),
            startTime: Math.min(merged.startTime, suite.startTime),
            endTime: Math.max(merged.endTime, suite.endTime),
            assertionResults: [
              ...merged.assertionResults,
              ...suite.assertionResults,
            ],
          },
    );
  }
  for (const [file, suite] of suites) {
    const run = group.runs.find(candidate => candidate.file === file);
    // Jest reports the tests a name pattern deselects as skipped, and a file
    // that failed to run as failed with no tests. Its name pattern can also
    // select tests the filters do not name, whose failures are left out.
    const suiteTestFailed = suite.assertionResults.some(
      test => test.status === 'failed',
    );
    const tests = filterResults(
      file,
      jestTestResults(file, suite, endedTime),
      [],
      {
        whole: run.whole,
        filters: run.filters,
        ended: suite.status !== 'failed' || suite.assertionResults.length > 0,
        endedTime,
        // Jest compiles `--testNamePattern` case-insensitively.
        flags: 'i',
      },
    );
    results.push(
      ...tests,
      testFileResult(
        file,
        tests,
        suite.status === 'failed' && !suiteTestFailed
          ? suite.message
          : undefined,
        suite.endTime > suite.startTime ? suite.endTime - suite.startTime : 0,
        endedTime,
      ),
    );
  }
  const notes = ['Jest reported no result for this file'];
  if (readError !== undefined) {
    notes.push(`could not read Jest's report: ${readError.message}`);
  }
  if (malformed > 0) {
    notes.push(`could not read ${malformed} test file entries in Jest's report`);
  }
  for (const file of files) {
    if (!suites.has(file)) {
      for (const note of notes) {
        printNote(`${file.name}: ${note}`);
      }
      const details = processDetails('jest', outcome, true, notes);
      results.push(testFileResult(file, [], details, 0, endedTime));
    }
  }
  const runFailures = [];
  // Jest exits with code 0 after a successful report. A report can be
  // unsuccessful with no failed file, as for a missed coverage threshold.
  // Without a report every file has already failed with the process details.
  if (
    report !== undefined &&
    (outcome.error !== undefined ||
      outcome.signal !== null ||
      (report.success && outcome.code !== 0) ||
      unselectedFailed.length > 0 ||
      (!report.success &&
        ![...suites.values()].some(suite => suite.status === 'failed')))
  ) {
    runFailures.push(
      processDetails(
        'jest',
        outcome,
        true,
        unselectedFailed.map(
          name => `Jest ran ${name}, which is not a test file of this run, and it failed`,
        ),
      ),
    );
  }
  return {results, runFailures};
}

async function runJestTests(options, runs, workDir) {
  const coverage = coveragePlan(workDir);
  // Jest applies a test name pattern to every file it runs, so only the files
  // run whole share a run. Each run already uses a worker per core but one, so
  // the runs go one at a time.
  const groups = [];
  const whole = runs.filter(run => run.whole);
  if (whole.length > 0) {
    groups.push({runs: whole});
  }
  for (const run of runs) {
    if (!run.whole) {
      groups.push({runs: [run]});
    }
  }
  const results = [];
  const runFailures = [];
  for (const [index, group] of groups.entries()) {
    try {
      const outcome = await runJestGroup(
        options,
        group,
        path.join(workDir, `jest-${index}.json`),
        coverage?.directory(index),
      );
      results.push(...outcome.results);
      runFailures.push(...outcome.runFailures);
    } catch (error) {
      const details = `the adapter failed during the Jest run that included this file: ${error?.stack ?? error}`;
      for (const {file} of group.runs) {
        printNote(`${file.name}: ${details}`);
        results.push(testFileResult(file, [], details, 0, Date.now()));
      }
      runFailures.push(details);
    }
  }
  runFailures.push(...writeCoverage(options, coverage, groups.length));
  return {results, runFailures};
}

function vitestArgs(options) {
  return [
    path.join(options.vitest, 'vitest', 'vitest.mjs'),
    '--root',
    options.root,
    ...(options.config === undefined ? [] : ['--config', options.config]),
    ...options.args,
  ];
}

async function vitestTestFiles(options, workDir) {
  const listFile = path.join(workDir, 'vitest-files.json');
  const outcome = await spawnNode(options.nodeArgs, [
    ...vitestArgs(options),
    'list',
    `--json=${listFile}`,
    '--filesOnly',
    ...options.files.map(file => file.path),
  ]);
  if (outcome.error !== undefined || outcome.code !== 0) {
    throw new AdapterError(processDetails('vitest list', outcome, true));
  }
  let listed;
  try {
    listed = JSON.parse(fs.readFileSync(listFile, 'utf8'));
  } catch (error) {
    throw new AdapterError(
      `vitest list wrote no test list: ${error.message}\n${processDetails('vitest list', outcome, false)}`,
    );
  }
  if (!Array.isArray(listed)) {
    throw new AdapterError('vitest list wrote no test list');
  }
  const findFile = fileFinder(options.files);
  const selected = new Set(
    listed
      .map(entry => findFile(entry?.file))
      .filter(file => file !== undefined),
  );
  const files = options.files.filter(file => selected.has(file));
  if (files.length === 0) {
    throw new AdapterError(
      `Vitest selects none of ${options.files.map(file => `'${file.name}'`).join(', ')}: check \`include\` and \`exclude\` in the Vitest config`,
    );
  }
  return files;
}

function isVitestSuite(suite) {
  return isJestSuite(suite);
}

function readVitestReport(reportFile) {
  const report = JSON.parse(fs.readFileSync(reportFile, 'utf8'));
  if (
    typeof report?.success !== 'boolean' ||
    !Array.isArray(report.testResults)
  ) {
    throw new Error('it is not a Vitest `--reporter=json` report');
  }
  return report;
}

function coveragePlan(workDir) {
  const output = process.env.COVERAGE_COLLECTION_OUTPUT_FILE;
  if (!output) {
    return undefined;
  }
  const reportsDirectory = path.resolve(
    process.env.COVERAGE_COLLECTION_TMPDIR ?? path.join(workDir, 'coverage'),
  );
  return {
    output: path.resolve(output),
    directory: index => path.join(reportsDirectory, String(index)),
  };
}

function coverageFilepath(sourcePath, options) {
  const root = path.resolve(options.root);
  const absolute = path.resolve(sourcePath);
  const relative = path.relative(root, absolute);
  if (
    relative === '' ||
    relative === '..' ||
    relative.startsWith(`..${path.sep}`) ||
    path.isAbsolute(relative)
  ) {
    return undefined;
  }
  const parts = relative.split(path.sep);
  if (parts.includes('node_modules')) {
    return undefined;
  }
  const staged = parts.join('/');
  const filepath =
    options.coveragePrefix === undefined
      ? staged
      : `${options.coveragePrefix.replace(/\/+$/, '')}/${staged}`;
  if (
    options.coveragePaths !== undefined &&
    !options.coveragePaths.some(
      selected =>
        filepath === selected ||
        filepath.startsWith(`${selected.replace(/\/+$/, '')}/`),
    )
  ) {
    return undefined;
  }
  return filepath;
}

function writeCoverage(options, plan, runCount) {
  if (plan === undefined) {
    return [];
  }
  const runFailures = [];
  const hitsByFile = new Map();
  for (let index = 0; index < runCount; index++) {
    let input;
    try {
      input = JSON.parse(
        fs.readFileSync(
          path.join(plan.directory(index), 'coverage-final.json'),
          'utf8',
        ),
      );
      if (typeof input !== 'object' || input === null || Array.isArray(input)) {
        throw new Error('coverage report is not an object');
      }
    } catch (error) {
      runFailures.push(`could not read coverage: ${error?.stack ?? error}`);
      continue;
    }
    for (const fileCoverage of Object.values(input)) {
      if (typeof fileCoverage?.path !== 'string') {
        continue;
      }
      const filepath = coverageFilepath(fileCoverage.path, options);
      if (filepath === undefined) {
        continue;
      }
      let lineHits = hitsByFile.get(filepath);
      if (lineHits === undefined) {
        lineHits = new Map();
        hitsByFile.set(filepath, lineHits);
      }
      for (const [id, statement] of Object.entries(fileCoverage.statementMap ?? {})) {
        const line = statement?.start?.line;
        if (typeof line !== 'number' || !Number.isInteger(line) || line < 1) {
          continue;
        }
        const hit = fileCoverage.s?.[id] ?? 0;
        const zeroBased = line - 1;
        lineHits.set(zeroBased, Math.max(lineHits.get(zeroBased) ?? 0, hit));
      }
    }
  }
  const entries = [...hitsByFile]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([filepath, lineHits]) => {
      const executable = [...lineHits.keys()].sort((a, b) => a - b);
      return {
        filepath,
        covered_lines: executable.filter(line => lineHits.get(line) > 0),
        executable_lines: executable,
      };
    });
  try {
    fs.mkdirSync(path.dirname(plan.output), {recursive: true});
    fs.writeFileSync(
      plan.output,
      entries.length === 0
        ? ''
        : entries.map(entry => JSON.stringify(entry)).join('\n') + '\n',
      'utf8',
    );
  } catch (error) {
    runFailures.push(`could not write coverage: ${error?.stack ?? error}`);
  }
  return runFailures;
}

async function runVitestGroup(options, group, reportFile, coverageDirectory) {
  const files = group.runs.map(run => run.file);
  const patterns = group.runs.flatMap(run =>
    run.whole ? [] : run.filters.map(filter => namePattern(run.file, filter, true)),
  );
  const outcome = await spawnNode(options.nodeArgs, [
    ...vitestArgs(options),
    'run',
    '--reporter=json',
    `--outputFile=${reportFile}`,
    ...(patterns.length > 0 ? [`--testNamePattern=${patterns.join('|')}`] : []),
    ...(coverageDirectory === undefined
      ? []
      : [
          '--coverage',
          '--coverage.reporter=json',
          `--coverage.reportsDirectory=${coverageDirectory}`,
          '--coverage.reportOnFailure=true',
        ]),
    ...files.map(file => file.path),
  ]);
  const endedTime = Date.now();
  let report;
  let readError;
  try {
    report = readVitestReport(reportFile);
  } catch (error) {
    readError = error;
  }
  const findFile = fileFinder(files);
  const results = [];
  const suites = new Map();
  const unselectedFailed = [];
  let malformed = 0;
  for (const suite of report?.testResults ?? []) {
    if (!isVitestSuite(suite)) {
      malformed++;
      continue;
    }
    const file = findFile(suite.name);
    if (file === undefined) {
      if (suite.status === 'failed') {
        unselectedFailed.push(suite.name);
      }
      continue;
    }
    const merged = suites.get(file);
    suites.set(
      file,
      merged === undefined
        ? suite
        : {
            status:
              merged.status === 'failed' || suite.status === 'failed'
                ? 'failed'
                : suite.status,
            message: [merged.message, suite.message]
              .filter(message => message !== '')
              .join('\n'),
            startTime: Math.min(merged.startTime, suite.startTime),
            endTime: Math.max(merged.endTime, suite.endTime),
            assertionResults: [
              ...merged.assertionResults,
              ...suite.assertionResults,
            ],
          },
    );
  }
  for (const [file, suite] of suites) {
    const run = group.runs.find(candidate => candidate.file === file);
    const suiteTestFailed = suite.assertionResults.some(
      test => test.status === 'failed',
    );
    const tests = filterResults(
      file,
      jestTestResults(file, suite, endedTime),
      [],
      {
        whole: run.whole,
        filters: run.filters,
        ended: suite.status !== 'failed' || suite.assertionResults.length > 0,
        endedTime,
      },
    );
    results.push(
      ...tests,
      testFileResult(
        file,
        tests,
        suite.status === 'failed' && !suiteTestFailed
          ? suite.message
          : undefined,
        suite.endTime > suite.startTime
          ? Math.round(suite.endTime - suite.startTime)
          : 0,
        endedTime,
      ),
    );
  }
  const notes = ['Vitest reported no result for this file'];
  if (readError !== undefined) {
    notes.push(`could not read Vitest's report: ${readError.message}`);
  }
  if (malformed > 0) {
    notes.push(`could not read ${malformed} test file entries in Vitest's report`);
  }
  for (const file of files) {
    if (!suites.has(file)) {
      for (const note of notes) {
        printNote(`${file.name}: ${note}`);
      }
      const details = processDetails('vitest', outcome, true, notes);
      results.push(testFileResult(file, [], details, 0, endedTime));
    }
  }
  const runFailures = [];
  if (
    report !== undefined &&
    (outcome.error !== undefined ||
      outcome.signal !== null ||
      (report.success && outcome.code !== 0) ||
      unselectedFailed.length > 0 ||
      (!report.success &&
        ![...suites.values()].some(suite => suite.status === 'failed')))
  ) {
    runFailures.push(
      processDetails(
        'vitest',
        outcome,
        true,
        unselectedFailed.map(
          name => `Vitest ran ${name}, which is not a test file of this run, and it failed`,
        ),
      ),
    );
  }
  return {results, runFailures};
}

async function runVitestTests(options, runs, workDir) {
  const coverage = coveragePlan(workDir);
  const groups = [];
  const whole = runs.filter(run => run.whole);
  if (whole.length > 0) {
    groups.push({runs: whole});
  }
  for (const run of runs) {
    if (!run.whole) {
      groups.push({runs: [run]});
    }
  }
  const results = [];
  const runFailures = [];
  for (const [index, group] of groups.entries()) {
    try {
      const outcome = await runVitestGroup(
        options,
        group,
        path.join(workDir, `vitest-${index}.json`),
        coverage?.directory(index),
      );
      results.push(...outcome.results);
      runFailures.push(...outcome.runFailures);
    } catch (error) {
      const details = `the adapter failed during the Vitest run that included this file: ${error?.stack ?? error}`;
      for (const {file} of group.runs) {
        printNote(`${file.name}: ${details}`);
        results.push(testFileResult(file, [], details, 0, Date.now()));
      }
      runFailures.push(details);
    }
  }
  runFailures.push(...writeCoverage(options, coverage, groups.length));
  return {results, runFailures};
}

function writeTestResults(events) {
  const output = process.env.TEST_RESULTS_OUTPUT_FILE;
  if (!output) {
    return;
  }
  fs.mkdirSync(path.dirname(output), {recursive: true});
  fs.writeFileSync(
    output,
    events.map(event => JSON.stringify(event) + '\n').join(''),
  );
}

function resultEvents(result) {
  const finish = {
    name: result.name,
    status: result.status,
    ended_time: result.endedTime,
    duration: result.duration,
  };
  if (result.message !== undefined) {
    finish.message = result.message;
  }
  return [
    {start: {name: result.name, started_time: result.endedTime - result.duration}},
    {finish},
  ];
}

function runFailureEvent(details) {
  return {run_failure: {status: RUN_FAILURE_FATAL, time: Date.now(), details}};
}

async function main(argv) {
  const options = parseArgs(argv);
  let workDir;
  killChildrenOnSignal(() => workDir);
  if (options.framework === 'jest') {
    options.files = await jestTestFiles(options);
  }
  if (options.framework === 'vitest') {
    workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'nodejs_test-'));
    try {
      options.files = await vitestTestFiles(options, workDir);
    } catch (error) {
      fs.rmSync(workDir, {recursive: true, force: true});
      throw error;
    }
  }
  if (options.list) {
    if (workDir !== undefined) {
      fs.rmSync(workDir, {recursive: true, force: true});
    }
    writeListing(options.files, options.output);
    return {code: 0};
  }
  const runs = selectRuns(options.files, options.filters);
  if (workDir === undefined) {
    workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'nodejs_test-'));
  }
  let outcome;
  try {
    outcome =
      options.framework === 'jest'
        ? await runJestTests(options, runs, workDir)
        : options.framework === 'vitest'
          ? await runVitestTests(options, runs, workDir)
          : await runNodeTests(options, runs, workDir);
  } finally {
    fs.rmSync(workDir, {recursive: true, force: true});
  }
  const {results, runFailures} = outcome;
  return {
    code:
      runFailures.length > 0 || results.some(result => result.status === FAIL)
        ? 1
        : 0,
    events: options.writeTestResults
      ? [...results.flatMap(resultEvents), ...runFailures.map(runFailureEvent)]
      : undefined,
  };
}

function describeError(error) {
  const {cause} = error;
  if (typeof cause?.stack !== 'string') {
    return error.message;
  }
  const inspected = util.inspect(cause);
  return error.failureType === 'testCodeFailure'
    ? inspected
    : `${error.message}\n${inspected}`;
}

async function* recordEvents(source) {
  for await (const {type, data} of source) {
    if (type === 'test:dequeue' || type === 'test:start') {
      yield JSON.stringify({
        type,
        name: data.name,
        nesting: data.nesting,
        file: data.file,
        line: data.line,
        column: data.column,
      }) + '\n';
    } else if (type === 'test:plan' && data.nesting === 0) {
      yield JSON.stringify({type}) + '\n';
    } else if (type === 'test:pass' || type === 'test:fail') {
      const {error} = data.details;
      yield JSON.stringify({
        type,
        name: data.name,
        nesting: data.nesting,
        file: data.file,
        line: data.line,
        column: data.column,
        suite: data.details.type === 'suite',
        durationMs: data.details.duration_ms,
        endedTime: Date.now(),
        skip: data.skip,
        todo: data.todo,
        failureType: error?.failureType,
        error: error == null ? undefined : describeError(error),
      }) + '\n';
    }
  }
}

if (require.main === module) {
  const argv = process.argv.slice(2);
  main(argv)
    .catch(error => {
      const details =
        error instanceof AdapterError ? error.message : util.inspect(error);
      console.error(`nodejs_test: ${details}`);
      return {
        code: 2,
        events: argv.includes('--write-test-results')
          ? [runFailureEvent(details)]
          : undefined,
      };
    })
    .then(({code, events}) => {
      process.exitCode = code;
      if (events === undefined) {
        return;
      }
      try {
        writeTestResults(events);
      } catch (writeError) {
        console.error(
          `nodejs_test: failed to write test results: ${util.inspect(writeError)}`,
        );
        process.exitCode = 2;
      }
    });
} else {
  module.exports = recordEvents;
}
