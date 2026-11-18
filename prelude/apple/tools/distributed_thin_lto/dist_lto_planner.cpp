/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#include <folly/DynamicConverter.h>
#include <folly/Subprocess.h>
#include <folly/coro/BlockingWait.h>
#include <folly/coro/Collect.h>
#include <folly/coro/SharedMutex.h>
#include <folly/coro/Task.h>
#include <folly/executors/GlobalExecutor.h>
#include <folly/executors/IOThreadPoolExecutor.h>
#include <folly/init/Init.h>
#include <folly/json.h>
#include <string.h>
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

namespace {

static std::string metaFileInputPath;
static std::string indexFileOutputPath;
static std::string linkPlanFileOutputPath;
static bool dumpLinkerInvocation = false;
static std::vector<std::string> linkerInvocationCommand;
folly::coro::SharedMutex nonLTOObjectFilesMutex;

struct ObjectFileRecord {
  const std::string inputObjectFilePath;
  const int starlarkArrayIndex;
  const std::string outputPlanFilePath;
  const std::string outputIndexShardFilePath;

  ObjectFileRecord(
      const std::string& inputObjectFilePath,
      const std::string& outputIndexShardFilePath,
      const std::string& outputPlanFilePath,
      const int starlarkArrayIndex)
      : inputObjectFilePath(inputObjectFilePath),
        starlarkArrayIndex(starlarkArrayIndex),
        outputPlanFilePath(outputPlanFilePath),
        outputIndexShardFilePath(outputIndexShardFilePath) {}
};

static const folly::json::serialization_opts jsonSerializationOptions = []() {
  folly::json::serialization_opts sorted;
  sorted.sort_keys = true;
  return sorted;
}();

static void parseArgs(int argc, char** argv) {
  int position = 1;
  const auto boundsCheckFlag = [&](const std::string& flag) {
    if (position + 1 >= argc) {
      std::cerr << "Missing argument for flag: " << flag << std::endl;
      std::abort();
    }
  };

  while (position < argc) {
    if (strcmp(argv[position], "--meta") == 0) {
      boundsCheckFlag("--meta");
      metaFileInputPath = argv[position + 1];
      position += 2;
    } else if (strcmp(argv[position], "--index") == 0) {
      boundsCheckFlag("--index");
      indexFileOutputPath = argv[position + 1];
      position += 2;
    } else if (strcmp(argv[position], "--link-plan") == 0) {
      boundsCheckFlag("--link-plan");
      linkPlanFileOutputPath = argv[position + 1];
      position += 2;
    } else if (strcmp(argv[position], "--dump-linker-invocation") == 0) {
      dumpLinkerInvocation = true;
      position += 1;
    } else if (strcmp(argv[position], "--") == 0) {
      for (position++; position < argc; position++) {
        linkerInvocationCommand.emplace_back(argv[position]);
      }
    } else {
      std::cerr << "Could not recognize argument: " << argv[position]
                << std::endl;
      std::abort();
    }
  }
}

enum class MetaFileRecordType {
  ArchiveMember,
  ObjectFile,
};

constexpr std::string_view SUMMARY_SHARD_SUFFIX = ".thinlto.bc";
constexpr std::string_view IMPORTS_FILE_SUFFIX = ".imports";

using ObjectFileRecordsMapTy =
    std::unordered_map<std::string, std::shared_ptr<const ObjectFileRecord>>;

static void parseMetaFileRecords(ObjectFileRecordsMapTy& objectFileRecordsMap) {
  std::ifstream metaFile(metaFileInputPath);
  if (!metaFile.is_open()) {
    std::cerr << "Failed to open file: " << metaFileInputPath << std::endl;
    std::abort();
  }
  std::stringstream buffer;
  buffer << metaFile.rdbuf();
  try {
    auto jsonData = folly::parseJson(buffer.str());
    for (const auto& record : jsonData) {
      std::shared_ptr<const ObjectFileRecord> parsedObjectFileRecord =
          std::make_shared<ObjectFileRecord>(
              record["input_object_file_path"].getString(),
              record["output_index_shard_file_path"].getString(),
              record["output_plan_file_path"].getString(),
              record["starlark_array_index"].getInt());
      objectFileRecordsMap[parsedObjectFileRecord->inputObjectFilePath] =
          parsedObjectFileRecord;
    }
  } catch (const folly::json::parse_error& e) {
    std::cerr << "Error parsing JSON: " << e.what() << std::endl;
    std::abort();
  }
}

static void runThinLink() {
  if (dumpLinkerInvocation) {
    for (const auto& argument : linkerInvocationCommand) {
      std::cout << argument << " ";
    }
    std::cout << "\n";
    return;
  }

  folly::Subprocess subprocess(
      linkerInvocationCommand, folly::Subprocess::Options().usePath());
  subprocess.waitChecked();
}

static std::filesystem::path thinLTOPrefixReplacedPath(
    const std::filesystem::path& path) {
  // The linker is passed --thinlto-prefix-replace=";<indexFileOutPath>" which
  // instructs the linker to prefix various paths with the value of
  // <indexFileOutPath>.
  return std::filesystem::path(indexFileOutputPath) / path;
}

static std::unordered_map<std::string, uint> parseLoadedObjectFiles() {
  std::unordered_map<std::string, uint> loadedObjectFilesPositions;
  std::ifstream stream(thinLTOPrefixReplacedPath("index.full"));
  std::string line;
  uint position = 0;
  while (std::getline(stream, line)) {
    // Paths to bitcode files will be prefixed with the argument to
    // --thinlto-prefix-replace, so we need to remove that prefix first to get a
    // normalized path.
    if (line.size() >= indexFileOutputPath.size() &&
        line.substr(0, indexFileOutputPath.size()) == indexFileOutputPath) {
      std::string normalizedPath = line.substr(indexFileOutputPath.size() + 1);
      loadedObjectFilesPositions[normalizedPath] = position;
    } else {
      loadedObjectFilesPositions[line] = position;
    }
    position++;
  }
  return loadedObjectFilesPositions;
}

static const std::vector<std::string> readImportsFile(
    const std::string& importsFilePath) {
  std::ifstream stream(importsFilePath);
  std::string line;
  std::vector<std::string> imports;
  while (std::getline(stream, line)) {
    imports.push_back(line);
  }
  return imports;
}

static void writeObjectFileOptimizationPlanToJSONFile(
    const std::vector<int>& objectFileImports,
    const bool isBitcode,
    const bool loadedByLinker,
    const std::string& outputPath,
    const int finalLinkLinePosition) {
  std::ofstream stream(outputPath);
  folly::dynamic json = folly::dynamic::object();
  json["imports"] = folly::toDynamic(objectFileImports);
  json["is_bitcode"] = isBitcode;
  json["loaded_by_linker"] = loadedByLinker;
  json["final_link_line_position"] = finalLinkLinePosition;
  stream << folly::json::serialize(json, jsonSerializationOptions);
}

static void writeLinkPlan(std::vector<int>& nonLTOObjectFiles) {
  std::ofstream stream(linkPlanFileOutputPath);
  folly::dynamic json = folly::dynamic::object;
  folly::dynamic nonLTOObjectsJson = folly::dynamic::object();
  for (const auto starlarkIndex : nonLTOObjectFiles) {
    nonLTOObjectsJson[std::to_string(starlarkIndex)] = 1;
  }
  json["non_lto_objects"] = nonLTOObjectsJson;
  stream << folly::json::serialize(json, jsonSerializationOptions);
}

static folly::coro::Task<void> writeOptimizationPlanForObjectFile(
    const std::string& inputObjectFilePath,
    const ObjectFileRecord* objectFile,
    const ObjectFileRecordsMapTy& objectFileRecordsMap,
    const std::unordered_map<std::string, uint>& loadedObjectFilesPositions,
    std::vector<int>& nonLTOObjectFiles) {
  co_await folly::coro::co_reschedule_on_current_executor;

  std::filesystem::create_directories(
      std::filesystem::path(objectFile->outputIndexShardFilePath)
          .parent_path());

  const std::string temporaryIndexShardFilePath =
      thinLTOPrefixReplacedPath(objectFile->inputObjectFilePath)
          .concat(SUMMARY_SHARD_SUFFIX);

  const std::string importsFilePath =
      thinLTOPrefixReplacedPath(objectFile->inputObjectFilePath)
          .concat(IMPORTS_FILE_SUFFIX);

  int finalLinkLinePosition = -1;
  auto it = loadedObjectFilesPositions.find(inputObjectFilePath);
  if (it != loadedObjectFilesPositions.end()) {
    finalLinkLinePosition = it->second;
  }

  if (std::filesystem::exists(importsFilePath)) {
    std::filesystem::rename(
        temporaryIndexShardFilePath, objectFile->outputIndexShardFilePath);

    const auto imports = readImportsFile(importsFilePath);

    std::vector<int> objectFileImports;
    for (const auto& importPath : imports) {
      const auto& objectFileRecord = objectFileRecordsMap.at(importPath);
      objectFileImports.push_back(objectFileRecord->starlarkArrayIndex);
    }

    writeObjectFileOptimizationPlanToJSONFile(
        objectFileImports,
        /* isBitcode */ true,
        /* loadedByLinker */
        loadedObjectFilesPositions.contains(inputObjectFilePath),
        objectFile->outputPlanFilePath,
        finalLinkLinePosition);
  } else {
    co_await nonLTOObjectFilesMutex.co_lock();
    nonLTOObjectFiles.push_back(objectFile->starlarkArrayIndex);
    nonLTOObjectFilesMutex.unlock();
    // The linker will not generate an index shard if the input is not bitcode.
    // Buck still expects the output, so write an empty file.
    std::ofstream(objectFile->outputIndexShardFilePath).close();

    writeObjectFileOptimizationPlanToJSONFile(
        /* objectFileImports */ {},
        /* isBitcode */ false,
        loadedObjectFilesPositions.contains(inputObjectFilePath),
        objectFile->outputPlanFilePath,
        finalLinkLinePosition);
  }
  co_return;
}

} // namespace

int main(int argc, char** argv) {
  int fakeArgc = 1;
  folly::Init init(&fakeArgc, &argv);
  parseArgs(argc, argv);
  ObjectFileRecordsMapTy objectFileRecordsMap;
  parseMetaFileRecords(objectFileRecordsMap);
  runThinLink();
  if (dumpLinkerInvocation) {
    return 0;
  }

  std::unordered_map<std::string, uint> loadedObjectFilesPositions =
      parseLoadedObjectFiles();

  std::vector<int> nonLTOObjectFiles;
  std::vector<folly::coro::Task<void>> tasks;
  for (const auto& [inputObjectFilePath, objectFileRecord] :
       objectFileRecordsMap) {
    tasks.push_back(writeOptimizationPlanForObjectFile(
        inputObjectFilePath,
        objectFileRecord.get(),
        objectFileRecordsMap,
        loadedObjectFilesPositions,
        nonLTOObjectFiles));
  }

  folly::coro::blockingWait(co_withExecutor(
      folly::getGlobalCPUExecutor(),
      folly::coro::collectAllRange(std::move(tasks))));

  writeLinkPlan(nonLTOObjectFiles);
}
