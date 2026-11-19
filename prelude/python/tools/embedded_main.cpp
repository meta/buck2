/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#include <Python.h>
#include <stdlib.h>
#include <sys/time.h>

#include <functional>
#include <optional>
#include <string>

namespace {

std::optional<int> MaybeGetExitCode(PyStatus* status, PyConfig* config) {
  if (PyStatus_IsExit(*status)) {
    return status->exitcode;
  }
  PyConfig_Clear(config);
  Py_ExitStatusException(*status);
}

} // namespace

extern struct _inittab _static_extension_info[];
PyMODINIT_FUNC PyInit__static_extension_utils();

int main(int argc, char* argv[]) {
  PyStatus status;
  PyConfig config;

  PyConfig_InitPythonConfig(&config);

  status = PyConfig_SetBytesString(&config, &config.program_name, argv[0]);
  if (PyStatus_Exception(status)) {
    if (auto exit_code = MaybeGetExitCode(&status, &config)) {
      return *exit_code;
    }
  }
#if PY_VERSION_HEX >= 0x030a0000 // 3.10
  status = PyConfig_SetBytesArgv(&config, argc, argv);
#else
  // Read all configuration at once.
  status = PyConfig_Read(&config);
#endif
  if (PyStatus_Exception(status)) {
    if (auto exit_code = MaybeGetExitCode(&status, &config)) {
      return *exit_code;
    }
  }

#if PY_VERSION_HEX >= 0x030a0000 // 3.10
  // Read all configuration at once.
  status = PyConfig_Read(&config);
#else
  status = PyConfig_SetBytesArgv(&config, argc, argv);
#endif
  if (PyStatus_Exception(status)) {
    if (auto exit_code = MaybeGetExitCode(&status, &config)) {
      return *exit_code;
    }
  }

  // Check if we're using par_style="native", if so, modify sys.path to include
  // the executable-zipfile to it, and set a main module to run when invoked.
#ifdef NATIVE_PAR_STYLE
  // Append path to the executable itself to sys.path.
  status =
      PyWideStringList_Append(&config.module_search_paths, config.executable);
  if (PyStatus_Exception(status)) {
    if (auto exit_code = MaybeGetExitCode(&status, &config)) {
      return *exit_code;
    }
  }

  // Run entry-point module at startup.
  status =
      PyConfig_SetBytesString(&config, &config.run_module, "__run_npar_main__");
  if (PyStatus_Exception(status)) {
    if (auto exit_code = MaybeGetExitCode(&status, &config)) {
      return *exit_code;
    }
  }
#endif /* NATIVE_PAR_STYLE */

  // TODO (T129253406) We can do some code generation on build, we will have tho
  // full library name and the symbol name. FYI the currently we mangle symbol
  // names to avoid collision so PyInit_bye ->
  // PyInit_python_efficiency_experimental_linking_tests_bye
  // One issue with this is how we would resolve foo_bar.baz and foo.bar_baz in
  // both cases the name would be mangled to PyInit_foo_bar_baz...
  if (auto exit_code = PyImport_AppendInittab(
          "_static_extension_utils", PyInit__static_extension_utils);
      exit_code != 0) {
    PyErr_Print();
    fprintf(stderr, "Error: could not update inittab\n");
    return exit_code;
  }

  if (std::getenv("FB_NATIVE_PYTHON_DEBUG")) {
    fprintf(
        stderr,
        "Pausing for debugger, pid=%d. Press <return> to continue.\n",
        (int)getpid());
    fflush(stderr);
    getchar();
  }

  status = Py_InitializeFromConfig(&config);
  if (PyStatus_Exception(status)) {
    if (auto exit_code = MaybeGetExitCode(&status, &config)) {
      return *exit_code;
    }
  }

  PyConfig_Clear(&config);
  return Py_RunMain();
}
