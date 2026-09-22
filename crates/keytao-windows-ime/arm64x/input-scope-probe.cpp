// Manual VM/disposable-account diagnostic, not an isolated automated test.
// RichEdit/TSF can activate an installed TIP despite ActivateEx flags. Module
// checks detect this after the fact; they cannot prevent initialization
// effects. Only our HWNDs receive messages; --enum reads class/PID/TID metadata
// only. See input-scope-probe.md before using --manual.
#define UNICODE
#define _UNICODE
#include <windows.h>

#include <inputscope.h>
#include <msctf.h>
#include <ole2.h>
#include <richedit.h>
#include <richole.h>
#include <stdio.h>
#include <string>
#include <textstor.h>
#include <tlhelp32.h>
#include <vector>
#include <wrl/client.h>
using Microsoft::WRL::ComPtr;
static unsigned probeCount = 0;
static HWND originalFocus;
static HKL originalLayout;
static std::vector<std::wstring> installedTipModules;

[[noreturn]] static void isolationFailure(const char *stage,
                                          const wchar_t *detail) {
  fprintf(stderr, "ISOLATION_FAILURE stage=%s detail=%ls\n", stage, detail);
  fflush(stderr);
  // Stop this helper immediately. Do not dispatch queued TSF activation or
  // continue probing after the isolation contract has failed.
  ExitProcess(8);
}

static std::wstring moduleName(const std::wstring &path) {
  const auto last = path.find_last_of(L"\\/");
  return path.substr(last == std::wstring::npos ? 0 : last + 1);
}

static void collectTipModules(HKEY hive) {
  HKEY tips = nullptr;
  LSTATUS status =
      RegOpenKeyExW(hive, L"SOFTWARE\\Microsoft\\CTF\\TIP", 0, KEY_READ, &tips);
  if (status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND)
    return;
  if (status != ERROR_SUCCESS)
    isolationFailure("registry", L"cannot enumerate registered TIPs");
  for (DWORD index = 0;; ++index) {
    wchar_t clsid[256] = {};
    DWORD length = ARRAYSIZE(clsid);
    status = RegEnumKeyExW(tips, index, clsid, &length, nullptr, nullptr,
                           nullptr, nullptr);
    if (status == ERROR_NO_MORE_ITEMS)
      break;
    if (status != ERROR_SUCCESS)
      isolationFailure("registry", L"cannot read registered TIP CLSID");
    const std::wstring key =
        std::wstring(L"CLSID\\") + clsid + L"\\InprocServer32";
    HKEY server = nullptr;
    status =
        RegOpenKeyExW(HKEY_CLASSES_ROOT, key.c_str(), 0, KEY_READ, &server);
    // A stale/out-of-process registration has no in-process DLL to load.
    if (status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND)
      continue;
    if (status != ERROR_SUCCESS)
      isolationFailure("registry", L"cannot inspect TIP in-process server");
    DWORD bytes = 0, type = 0;
    status = RegQueryValueExW(server, nullptr, nullptr, &type, nullptr, &bytes);
    if (status != ERROR_SUCCESS || (type != REG_SZ && type != REG_EXPAND_SZ))
      isolationFailure("registry", L"invalid TIP in-process server value");
    std::vector<wchar_t> path(bytes / sizeof(wchar_t) + 1, 0);
    status = RegQueryValueExW(server, nullptr, nullptr, &type,
                              reinterpret_cast<BYTE *>(path.data()), &bytes);
    RegCloseKey(server);
    if (status != ERROR_SUCCESS)
      isolationFailure("registry", L"cannot read TIP in-process server value");
    std::wstring file(path.data());
    if (type == REG_EXPAND_SZ) {
      DWORD needed = ExpandEnvironmentStringsW(file.c_str(), nullptr, 0);
      if (!needed)
        isolationFailure("registry", L"cannot expand TIP module path");
      std::vector<wchar_t> expanded(needed, 0);
      if (ExpandEnvironmentStringsW(file.c_str(), expanded.data(), needed) !=
          needed)
        isolationFailure("registry", L"TIP path changed while expanding");
      file = expanded.data();
    }
    if (file.size() >= 2 && file.front() == L'"' && file.back() == L'"')
      file = file.substr(1, file.size() - 2);
    if (file.empty())
      isolationFailure("registry", L"empty TIP module path");
    installedTipModules.push_back(moduleName(file));
  }
  RegCloseKey(tips);
}

static void checkIsolation(const char *stage) {
  if (GetFocus() != originalFocus || GetKeyboardLayout(0) != originalLayout)
    isolationFailure(stage, L"thread focus or keyboard layout changed");
  HANDLE snapshot = CreateToolhelp32Snapshot(
      TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, GetCurrentProcessId());
  if (snapshot == INVALID_HANDLE_VALUE)
    isolationFailure(stage, L"cannot enumerate helper modules");
  MODULEENTRY32W entry = {sizeof(entry)};
  if (!Module32FirstW(snapshot, &entry))
    isolationFailure(stage, L"cannot read helper module snapshot");
  do {
    const bool rime = _wcsicmp(entry.szModule, L"rime.dll") == 0 ||
                      _wcsicmp(entry.szModule, L"rime-arm64.dll") == 0;
    bool tip = _wcsicmp(entry.szModule, L"keytao_windows_ime.dll") == 0;
    for (const auto &registered : installedTipModules)
      tip = tip || _wcsicmp(entry.szModule, registered.c_str()) == 0;
    if (rime || tip)
      isolationFailure(stage, entry.szModule);
  } while (Module32NextW(snapshot, &entry));
  if (GetLastError() != ERROR_NO_MORE_FILES)
    isolationFailure(stage, L"incomplete helper module snapshot");
  CloseHandle(snapshot);
  printf("isolation stage=%s rime_loaded=false registered_tip_loaded=false\n",
         stage);
  fflush(stdout);
}

static const CLSID kThreadMgr = {
    0x529a9e6b,
    0x6587,
    0x4f23,
    {0xab, 0x9e, 0x9c, 0x7d, 0x68, 0x3e, 0x3c, 0x50}};
static const GUID kInputScope = {
    0x1713dd5a,
    0x68e7,
    0x4a5b,
    {0x9a, 0xf6, 0x59, 0x2a, 0x59, 0x5c, 0x77, 0x8d}};
static void result(const char *op, HRESULT hr) {
  printf("%s hr=0x%08lX\n", op, (unsigned long)hr);
  fflush(stdout);
}
static BOOL CALLBACK child(HWND hwnd, LPARAM target) {
  DWORD pid = 0;
  DWORD tid = GetWindowThreadProcessId(hwnd, &pid);
  if (pid != (DWORD)target)
    return TRUE;
  wchar_t cls[256] = {};
  GetClassNameW(hwnd, cls, 256);
  printf("hwnd=0x%llX pid=%lu tid=%lu class=",
         (unsigned long long)(UINT_PTR)hwnd, pid, tid);
  wprintf(L"%ls\n", cls);
  fflush(stdout);
  return TRUE;
}
static BOOL CALLBACK top(HWND hwnd, LPARAM target) {
  DWORD pid = 0;
  GetWindowThreadProcessId(hwnd, &pid);
  if (pid == (DWORD)target) {
    child(hwnd, target);
    EnumChildWindows(hwnd, child, target);
  }
  return TRUE;
}
static void valueProbe(ITfReadOnlyProperty *prop, TfEditCookie ec,
                       ITfRange *range, const char *label) {
  printf("property_range=%s\n", label);
  VARIANT value;
  VariantInit(&value);
  HRESULT hr = prop->GetValue(ec, range, &value);
  result("GetValue", hr);
  printf("vt=%u\n", (unsigned)value.vt);
  if (SUCCEEDED(hr) && value.vt == VT_UNKNOWN && value.punkVal) {
    ComPtr<ITfInputScope> scopes;
    hr = value.punkVal->QueryInterface(IID_PPV_ARGS(&scopes));
    result("QI(ITfInputScope)", hr);
    if (SUCCEEDED(hr)) {
      InputScope *values = nullptr;
      UINT count = 0;
      hr = scopes->GetInputScopes(&values, &count);
      result("GetInputScopes", hr);
      printf("scope_count=%u scope_values=", count);
      for (UINT i = 0; values && i < count; i++)
        printf("%s%d", i ? "," : "", (int)values[i]);
      printf("\n");
      CoTaskMemFree(values);
    }
  }
  VariantClear(&value);
  fflush(stdout);
}
static void probe(ITfContext *context, TfEditCookie ec) {
  ++probeCount;
  TF_STATUS status = {};
  HRESULT hr = context->GetStatus(&status);
  result("GetStatus", hr);
  printf("static_flags=0x%08lX dynamic_flags=0x%08lX\n", status.dwStaticFlags,
         status.dwDynamicFlags);
  ComPtr<ITfContextView> view;
  hr = context->GetActiveView(&view);
  result("GetActiveView", hr);
  if (view) {
    HWND hwnd = nullptr;
    hr = view->GetWnd(&hwnd);
    result("GetWnd", hr);
    DWORD pid = 0;
    DWORD tid = GetWindowThreadProcessId(hwnd, &pid);
    wchar_t cls[256] = {};
    GetClassNameW(hwnd, cls, 256);
    printf("view_hwnd=0x%llX pid=%lu tid=%lu same_thread=%d class=",
           (unsigned long long)(UINT_PTR)hwnd, pid, tid,
           tid == GetCurrentThreadId());
    wprintf(L"%ls\n", cls);
    if (pid == GetCurrentProcessId() && tid == GetCurrentThreadId()) {
      printf("style=0x%08llX password_char=%lld\n",
             (unsigned long long)GetWindowLongPtrW(hwnd, GWL_STYLE),
             (long long)SendMessageW(hwnd, EM_GETPASSWORDCHAR, 0, 0));
    }
  }
  ComPtr<ITfReadOnlyProperty> prop;
  hr = context->GetAppProperty(kInputScope, &prop);
  result("GetAppProperty(INPUTSCOPE)", hr);
  printf("property=%s\n", prop ? "nonnull" : "null");
  TF_SELECTION selection = {};
  ULONG fetched = 0;
  HRESULT selectionHr =
      context->GetSelection(ec, TF_DEFAULT_SELECTION, 1, &selection, &fetched);
  result("GetSelection", selectionHr);
  printf("fetched=%lu range=%s\n", fetched,
         selection.range ? "nonnull" : "null");
  ComPtr<ITfRange> range;
  range.Attach(selection.range);
  if (FAILED(hr) || !prop || FAILED(selectionHr) || fetched == 0 || !range)
    return;
  valueProbe(prop.Get(), ec, range.Get(), "selection");
  LONG shifted = 0;
  hr = range->ShiftEnd(ec, 1, &shifted, nullptr);
  result("selection.ShiftEnd(+1)", hr);
  printf("shifted=%ld\n", shifted);
  valueProbe(prop.Get(), ec, range.Get(), "selection_plus_one");
  ComPtr<ITfRange> start;
  hr = context->GetStart(ec, &start);
  result("GetStart", hr);
  if (start) {
    valueProbe(prop.Get(), ec, start.Get(), "start_collapsed");
    start->ShiftEnd(ec, 1, &shifted, nullptr);
    valueProbe(prop.Get(), ec, start.Get(), "start_plus_one");
  }
}
class EditSession : public ITfEditSession {
  LONG refs = 1;
  ComPtr<ITfContext> context;

public:
  explicit EditSession(ITfContext *c) : context(c) {}
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **ptr) override {
    if (!ptr)
      return E_POINTER;
    *ptr = nullptr;
    if (iid == __uuidof(IUnknown) || iid == __uuidof(ITfEditSession)) {
      *ptr = this;
      AddRef();
      return S_OK;
    }
    return E_NOINTERFACE;
  }
  ULONG STDMETHODCALLTYPE AddRef() override {
    return InterlockedIncrement(&refs);
  }
  ULONG STDMETHODCALLTYPE Release() override {
    LONG n = InterlockedDecrement(&refs);
    if (!n)
      delete this;
    return n;
  }
  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie ec) override {
    printf("DoEditSession ec=%lu\n", ec);
    probe(context.Get(), ec);
    return S_OK;
  }
};
static void contexts(ITfThreadMgr *mgr, TfClientId cid, HWND ownEdit) {
  ComPtr<IEnumTfDocumentMgrs> en;
  HRESULT hr = mgr->EnumDocumentMgrs(&en);
  result("EnumDocumentMgrs", hr);
  if (FAILED(hr))
    return;
  ULONG n = 0;
  unsigned docs = 0;
  while (true) {
    ComPtr<ITfDocumentMgr> doc;
    hr = en->Next(1, &doc, &n);
    if (hr != S_OK || n == 0)
      break;
    docs++;
    ComPtr<ITfContext> ctx;
    hr = doc->GetTop(&ctx);
    result("Document.GetTop", hr);
    if (FAILED(hr) || !ctx)
      continue;
    // Password controls can also create a transient TSF document. Inspect only
    // the context whose active view is this fixture's current RichEdit window.
    ComPtr<ITfContextView> view;
    HWND hwnd = nullptr;
    if (FAILED(ctx->GetActiveView(&view)) || !view ||
        FAILED(view->GetWnd(&hwnd)) || hwnd != ownEdit) {
      printf("skip_context_without_matching_fixture_view\n");
      continue;
    }
    ComPtr<ITfEditSession> edit;
    edit.Attach(new EditSession(ctx.Get()));
    HRESULT sessionHr = E_FAIL;
    hr = ctx->RequestEditSession(cid, edit.Get(), TF_ES_SYNC | TF_ES_READ,
                                 &sessionHr);
    result("RequestEditSession", hr);
    result("session_result", sessionHr);
  }
  printf("document_count=%u\n", docs);
  fflush(stdout);
}
int wmain(int argc, wchar_t **argv) {
  if (argc > 2 && wcscmp(argv[1], L"--enum") == 0) {
    EnumWindows(top, (LPARAM)wcstoul(argv[2], nullptr, 10));
    return 0;
  }
  if (argc < 2 || wcscmp(argv[1], L"--manual") != 0) {
    fprintf(stderr, "Manual VM/disposable-account use only: probe.exe --manual "
                    "[absolute RichEdit DLL]\n"
                    "RichEdit may activate an installed TIP; module checks "
                    "detect but do not prevent it.\n"
                    "Read-only window metadata: probe.exe --enum PID\n");
    return 64;
  }
  originalFocus = GetFocus();
  originalLayout = GetKeyboardLayout(0);
  collectTipModules(HKEY_LOCAL_MACHINE);
  collectTipModules(HKEY_CURRENT_USER);
  checkIsolation("startup");
  HRESULT hr = OleInitialize(nullptr);
  result("OleInitialize", hr);
  if (FAILED(hr))
    return 1;
  checkIsolation("ole_initialize");
  ComPtr<ITfThreadMgr> mgr;
  hr = CoCreateInstance(kThreadMgr, nullptr, CLSCTX_INPROC_SERVER,
                        IID_PPV_ARGS(&mgr));
  result("CreateThreadMgr", hr);
  if (FAILED(hr))
    return 2;
  ComPtr<ITfThreadMgrEx> ex;
  hr = mgr.As(&ex);
  result("QI(ThreadMgrEx)", hr);
  if (FAILED(hr))
    return 3;
  TfClientId cid = 0;
  // These flags defer activation during this call only. TSF may activate a TIP
  // asynchronously later, so this helper NEVER pumps messages and checks real
  // module mappings after every phase. Flags alone do not provide isolation.
  hr = ex->ActivateEx(&cid,
                      TF_TMAE_NOACTIVATETIP | TF_TMAE_NOACTIVATEKEYBOARDLAYOUT);
  result("ActivateEx(no_tip,no_keyboard_layout)", hr);
  if (FAILED(hr))
    return 4;
  checkIsolation("activate_ex");
  wchar_t systemDir[MAX_PATH] = {};
  if (!GetSystemDirectoryW(systemDir, MAX_PATH))
    return 5;
  const std::wstring defaultDll = std::wstring(systemDir) + L"\\msftedit.dll";
  const wchar_t *dll = argc > 2 ? argv[2] : defaultDll.c_str();
  HMODULE rich = LoadLibraryExW(dll, nullptr,
                                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR |
                                    LOAD_LIBRARY_SEARCH_SYSTEM32);
  printf("LoadLibrary error=%lu module=%s\n", GetLastError(),
         rich ? "nonnull" : "null");
  if (!rich)
    return 5;
  checkIsolation("load_richedit");
  HWND parent = CreateWindowExW(WS_EX_NOACTIVATE, L"STATIC",
                                L"KeyTao isolated scope fixture", WS_OVERLAPPED,
                                0, 0, 600, 400, nullptr, nullptr,
                                GetModuleHandleW(nullptr), nullptr);
  if (!parent)
    return 6;
  const wchar_t *classes[] = {L"RICHEDIT50W", L"RichEditD2DPT", L"RichEditD2D",
                              L"RICHEDIT60W", L"RichEdit20W"};
  typedef HRESULT(WINAPI * SetScopeFn)(HWND, InputScope);
  auto setScope = (SetScopeFn)GetProcAddress(GetModuleHandleW(L"msctf.dll"),
                                             "SetInputScope");
  printf("SetInputScope available=%d\n", setScope != nullptr);
  for (auto cls : classes) {
    WNDCLASSEXW wc = {sizeof(wc)};
    BOOL exists =
        GetClassInfoExW(rich, cls, &wc) || GetClassInfoExW(nullptr, cls, &wc);
    wprintf(L"class %ls exists=%d\n", cls, exists);
    if (!exists)
      continue;
    for (unsigned scenario = 0; scenario < 5; scenario++) {
      const char *labels[] = {"normal_nonempty", "scope_default",
                              "scope_password", "style_password",
                              "normal_empty"};
      printf("scenario=%s\n", labels[scenario]);
      HWND edit = CreateWindowExW(
          0, cls, scenario == 4 ? L"" : L"fixture",
          WS_CHILD | ES_MULTILINE | (scenario == 3 ? ES_PASSWORD : 0), 0, 0,
          500, 300, parent, nullptr, GetModuleHandleW(nullptr), nullptr);
      printf("CreateRichEdit hwnd=%s error=%lu\n", edit ? "nonnull" : "null",
             GetLastError());
      if (!edit)
        continue;
      checkIsolation("create_control");
      LRESULT style =
          SendMessageW(edit, EM_SETEDITSTYLE, SES_USECTF, SES_USECTF);
      printf("EM_SETEDITSTYLE returned=0x%llX\n", (unsigned long long)style);
      if (scenario == 3)
        SendMessageW(edit, EM_SETPASSWORDCHAR, L'*', 0);
      // Initialize RichEdit's TSF document without showing a window, changing
      // actual keyboard focus, or sending input. This is our own child HWND.
      // It publishes a synchronous context; DO NOT dispatch queued messages
      // afterward, because that can activate an installed TIP asynchronously.
      SendMessageW(edit, WM_SETFOCUS, 0, 0);
      checkIsolation("control_focus_notification");
      if (scenario == 1 && setScope)
        result("SetInputScope(DEFAULT) after_focus",
               setScope(edit, IS_DEFAULT));
      if (scenario == 2 && setScope)
        result("SetInputScope(PASSWORD) after_focus",
               setScope(edit, IS_PASSWORD));
      checkIsolation("set_scope");
      ComPtr<IRichEditOle> richOle;
      LRESULT got = SendMessageW(edit, EM_GETOLEINTERFACE, 0,
                                 (LPARAM)richOle.GetAddressOf());
      printf("EM_GETOLEINTERFACE result=%lld ptr=%s\n", (long long)got,
             richOle ? "nonnull" : "null");
      if (richOle) {
        ComPtr<ITextStoreACP> store;
        result("RichEdit QI(ITextStoreACP)", richOle.As(&store));
        ComPtr<ITfContext> ctx;
        result("RichEdit QI(ITfContext)", richOle.As(&ctx));
      }
      contexts(mgr.Get(), cid, edit);
      checkIsolation("read_scope");
      richOle.Reset();
      if ((scenario == 1 || scenario == 2) && setScope)
        result("SetInputScope(DEFAULT) cleanup", setScope(edit, IS_DEFAULT));
      SendMessageW(edit, WM_KILLFOCUS, 0, 0);
      DestroyWindow(edit);
      checkIsolation("destroy_control");
    }
  }
  DestroyWindow(parent);
  ex->Deactivate();
  mgr.Reset();
  ex.Reset();
  OleUninitialize();
  checkIsolation("cleanup");
  printf("completed_read_sessions=%u\n", probeCount);
  // Property-read failures are observations, not harness failures. Zero edit
  // sessions means this run never reached the API under investigation.
  return probeCount ? 0 : 7;
}
