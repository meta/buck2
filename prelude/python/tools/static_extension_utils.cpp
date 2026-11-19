/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#include <stdexcept>
#include <string>
#include <string_view>
#include <unordered_map>
#include "Python.h"

typedef PyObject* (*pyinitfunc)();

typedef struct {
} StaticExtensionFinderObject;

// This map is generated at build time by generate_static_extension_info.py
// and should not be modified at runtime.
extern std::unordered_map<std::string_view, pyinitfunc> _static_extension_info;

namespace {

#if PY_VERSION_HEX < 0x030E0000
static PyObject* _handle_single_phase_initialization(
    PyObject* mod,
    PyObject* name,
    PyObject* spec,
    pyinitfunc initfunc,
    PyObject* modules) {
  /* Remember pointer to module init function. */
  PyModuleDef* def = PyModule_GetDef(mod);
  if (def == nullptr) {
    Py_DECREF(name);
    return nullptr;
  }
  PyObject* path = PyObject_GetAttrString(spec, "origin");
  if (PyModule_AddObject(mod, "__file__", path) < 0) {
    PyErr_Clear();
  } else {
    Py_INCREF(path);
  }
  def->m_base.m_init = initfunc;
  if (modules == nullptr) {
    modules = PyImport_GetModuleDict();
  }

  int result = _PyImport_FixupExtensionObject(mod, name, name, modules);
  Py_DECREF(path);
  Py_DECREF(name);
  if (result < 0) {
    return nullptr;
  }
  return mod;
}
#endif

static PyObject* _create_module(PyObject* self, PyObject* spec) {
  PyObject* name;
  PyObject* mod;

  name = PyObject_GetAttrString(spec, "name");
  if (name == nullptr) {
    return nullptr;
  }
  // Attempt to import the module normally (builtins, etc)
  mod = PyImport_GetModule(name);
  if (mod || PyErr_Occurred()) {
    Py_DECREF(name);
    Py_XINCREF(mod);
    return mod;
  }

  // If the module is not found, try to find it in the static extension map
  // its generated at build time by generate_static_extension_info.py
  const std::string namestr = PyUnicode_AsUTF8(name);
  if (namestr.empty()) {
    Py_DECREF(name);
    return nullptr;
  }

  pyinitfunc initfunc = nullptr;
  if (auto result = _static_extension_info.find(namestr);
      result != _static_extension_info.end())
    initfunc = result->second;

  if (initfunc == nullptr) {
    PyErr_SetString(
        PyExc_ImportError, "Module unknown to static extension finder");
    return nullptr;
  }

#if PY_VERSION_HEX >= 0x030E0000
  // Python 3.14+ uses PyImport_CreateModuleFromInitfunc. This API was
  // upstreamed in 3.15 (gh-116146) and backported to Meta Python 3.14
  // (patched/Include/cpython/import.h). The older Meta-specific
  // _Ci_PyImport_CreateBuiltinFromSpecAndInitfunc is now obsolete and
  // can be avoided.
  mod = PyImport_CreateModuleFromInitfunc(spec, initfunc);
#elif PY_VERSION_HEX >= 0x030C0000
  // Python 3.12-3.13: directly call the init function. Meta Python 3.12
  // exposes a custom C-API for package context handling; there is no
  // supported Meta/Cinder Python 3.13 build, so 3.13 falls back to the stock
  // path.
#if defined(META_PYTHON)
  mod = _Ci_PyImport_CallInitFuncWithContext(namestr.c_str(), initfunc);
#else
  mod = initfunc();
#endif
#else
  throw std::runtime_error(
      "Native python does not support Python 3.11 and earlier.");
#endif
  if (mod == nullptr) {
    Py_DECREF(name);
    return nullptr;
  }
#if PY_VERSION_HEX < 0x030E0000
  // If the result is a PyModuleDef, then this is multi-phase initialization
  // Return the PyModule so module_exec can be called on it later
  if (PyObject_TypeCheck(mod, &PyModuleDef_Type)) {
    Py_DECREF(name);
    return PyModule_FromDefAndSpec((PyModuleDef*)mod, spec);
  }
  PyObject* modules = nullptr;
  // At this point we know this is single-phase initialization
  return _handle_single_phase_initialization(
      mod, name, spec, initfunc, modules);
#else
  return mod;
#endif
}

static PyObject* _exec_module(PyObject* self, PyObject* module) {
  PyModuleDef* def;

  // TODO errors
  if (!PyModule_Check(module)) {
    // TODO
    Py_RETURN_NONE;
  }

  def = PyModule_GetDef(module);
  if (def == nullptr) {
    // TODO
    Py_RETURN_NONE;
  }

  // TODO check this result
  PyModule_ExecDef(module, def);

  Py_RETURN_NONE;
}

static PyObject* _get_source(PyObject* self, PyObject* /* module_name */) {
  // TODO properly implement the function
  Py_RETURN_NONE;
}

PyDoc_STRVAR(
    StaticExtensionLoader_doc,
    "static_extension_loader(name: str)\n\
\n\
a loader for extensions linked statically into the binary");

static PyMethodDef StaticExtensionLoaderType_methods[] = {
    {"create_module",
     (PyCFunction)(void (*)(void))_create_module,
     METH_STATIC | METH_O,
     nullptr},
    {"exec_module",
     (PyCFunction)(void (*)(void))_exec_module,
     METH_STATIC | METH_O,
     nullptr},
    {"get_source",
     (PyCFunction)(void (*)(void))_get_source,
     METH_STATIC | METH_O,
     nullptr},
    {}};

static PyType_Slot StaticExtensionLoader_slots[] = {
    {Py_tp_doc, (void*)StaticExtensionLoader_doc},
    {Py_tp_methods, StaticExtensionLoaderType_methods},
    {}};

static PyType_Spec StaticExtensionLoader_spec = {
    "static_extension_utils.StaticExtensionLoader",
    0,
    0,
    Py_TPFLAGS_DEFAULT,
    StaticExtensionLoader_slots};

static int _static_extension_utils_exec(PyObject* m) {
  PyObject* loader_type = PyType_FromSpec(&StaticExtensionLoader_spec);
  if (loader_type == nullptr) {
    return -1;
  }
  int result = PyModule_AddObject(m, "StaticExtensionLoader", loader_type);
  if (result == -1) {
    Py_DECREF(loader_type);
    return -1;
  }
  return 0;
}

PyDoc_STRVAR(
    _check_module_doc,
    "Check if a module is contained in the C Extension map \n");

static PyObject* _check_module(PyObject* self, PyObject* fullname) {
  if (!PyUnicode_Check(fullname)) {
    PyErr_SetString(PyExc_TypeError, "Expected a unicode object");
    return nullptr;
  }
  const std::string modname = PyUnicode_AsUTF8(fullname);
  if (_static_extension_info.find(modname) != _static_extension_info.end()) {
    Py_INCREF(Py_True);
    return Py_True;
  }
  Py_INCREF(Py_False);
  return Py_False;
}

static PyModuleDef_Slot _static_extension_utils_slots[] = {
    {Py_mod_exec, (void*)_static_extension_utils_exec},
#ifdef Py_GIL_DISABLED
    {Py_mod_gil, Py_MOD_GIL_NOT_USED},
#endif
    {}};

static PyMethodDef _static_extension_utils_methods[] = {
    {"_check_module", _check_module, METH_O, _check_module_doc},
    {}};

PyDoc_STRVAR(
    module_doc,
    "Utils for importing modules statically linked into the python binary \n");

static struct PyModuleDef _static_extension_utils_def = {
    PyModuleDef_HEAD_INIT,
    "_static_extension_utils_def",
    module_doc,
    0,
    _static_extension_utils_methods,
    _static_extension_utils_slots,
    nullptr,
    nullptr,
    nullptr};

PyMODINIT_FUNC PyInit__static_extension_utils(void) {
  return PyModuleDef_Init(&_static_extension_utils_def);
}
} // namespace
