// Run with one absolute TIP DLL path. This creates its own temporary sandbox;
// it never registers/activates an input profile or injects into a real host.
// cl /nologo /std:c++17 /EHsc /W4 /MT appcontainer-load-test.cpp
//    /link userenv.lib advapi32.lib ole32.lib shell32.lib
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <aclapi.h>
#include <sddl.h>
#include <shlobj.h>
#include <userenv.h>
#include <cstdio>
#include <string>
#include <vector>

namespace {
const CLSID kTextService = {0x4A5C6D7E, 0x8F90, 0x1A2B,
                           {0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B, 0x9C, 0x0D}};

// Exit codes identify the failing assertion even without inheriting parent
// handles into the restricted process.
int Child(const wchar_t* dll) {
  SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX |
               SEM_NOOPENFILEERRORBOX);
  HANDLE token = nullptr;
  DWORD sandboxed = 0, returned = 0;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return 10;
  const BOOL queried = GetTokenInformation(token, TokenIsAppContainer,
                                           &sandboxed, sizeof(sandboxed), &returned);
  CloseHandle(token);
  if (!queried || sandboxed != 1) return 11;

  const HRESULT initialized = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
  if (FAILED(initialized)) return 12;
  PWSTR private_path = nullptr, desktop_path = nullptr;
  HRESULT result = SHGetKnownFolderPath(FOLDERID_LocalAppData,
      KF_FLAG_FORCE_PACKAGE_REDIRECTION, nullptr, &private_path);
  if (SUCCEEDED(result)) {
    result = SHGetKnownFolderPath(FOLDERID_LocalAppData,
        KF_FLAG_NO_PACKAGE_REDIRECTION | KF_FLAG_DONT_VERIFY, nullptr, &desktop_path);
  }
  if (FAILED(result) || !private_path || !desktop_path) {
    CoTaskMemFree(private_path);
    CoTaskMemFree(desktop_path);
    CoUninitialize();
    return 13;
  }
  const bool redirected = _wcsicmp(private_path, desktop_path) != 0;
  const std::wstring probe = std::wstring(private_path) + L"\\KeyTaoSmoke.tmp";
  CoTaskMemFree(private_path);
  CoTaskMemFree(desktop_path);
  CoUninitialize();
  if (!redirected) return 14;
  HANDLE file = CreateFileW(probe.c_str(), GENERIC_WRITE, 0, nullptr,
      CREATE_NEW, FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE, nullptr);
  if (file == INVALID_HANDLE_VALUE) return 15;
  const char value[] = "private-cache-is-writable";
  DWORD written = 0;
  const BOOL wrote = WriteFile(file, value, sizeof(value), &written, nullptr);
  CloseHandle(file);
  if (!wrote || written != sizeof(value)) return 16;

  HANDLE mutex = CreateMutexW(nullptr, FALSE, L"KeyTao.WindowsIme.EngineInit");
  if (!mutex) return 22;
  const DWORD acquired = WaitForSingleObject(mutex, 1000);
  if (acquired == WAIT_OBJECT_0 || acquired == WAIT_ABANDONED) ReleaseMutex(mutex);
  CloseHandle(mutex);
  if (acquired != WAIT_OBJECT_0 && acquired != WAIT_ABANDONED) return 23;

  HANDLE runtime_write = CreateFileW(dll, GENERIC_WRITE,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (runtime_write != INVALID_HANDLE_VALUE) {
    CloseHandle(runtime_write);
    return 24;
  }
  if (GetLastError() != ERROR_ACCESS_DENIED) return 25;

  // Prohibit rime.dll being found through the current directory or PATH.
  if (!SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)) return 17;
  HMODULE module = LoadLibraryExW(dll, nullptr,
      LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
  if (!module) return 18;
  using GetClassObject = HRESULT(STDAPICALLTYPE*)(REFCLSID, REFIID, void**);
  const auto get_class = reinterpret_cast<GetClassObject>(
      GetProcAddress(module, "DllGetClassObject"));
  if (!get_class) return 19;
  IClassFactory* factory = nullptr;
  result = get_class(kTextService, IID_IClassFactory,
                     reinterpret_cast<void**>(&factory));
  if (FAILED(result) || !factory) return 20;
  Sleep(2000); // Let the asynchronous runtime-log writer start.
  const bool engine_loaded = GetModuleHandleW(L"rime.dll") != nullptr;
  factory->Release();
  // Keep the DLL mapped until process exit: its logger thread owns DLL work.
  return engine_loaded ? 21 : 0;
}

DWORD GrantReadExecute(const std::wstring& path, PSID sid) {
  PACL old_acl = nullptr, new_acl = nullptr;
  PSECURITY_DESCRIPTOR security = nullptr;
  DWORD error = GetNamedSecurityInfoW(path.c_str(), SE_FILE_OBJECT,
      DACL_SECURITY_INFORMATION, nullptr, nullptr, &old_acl, nullptr, &security);
  if (error != ERROR_SUCCESS) return error;
  EXPLICIT_ACCESSW access{};
  access.grfAccessPermissions = GENERIC_READ | GENERIC_EXECUTE;
  access.grfAccessMode = GRANT_ACCESS;
  access.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
  access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
  access.Trustee.TrusteeType = TRUSTEE_IS_WELL_KNOWN_GROUP;
  access.Trustee.ptstrName = static_cast<LPWSTR>(sid);
  error = SetEntriesInAclW(1, &access, old_acl, &new_acl);
  if (error == ERROR_SUCCESS) {
    error = SetNamedSecurityInfoW(const_cast<LPWSTR>(path.c_str()), SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION, nullptr, nullptr, new_acl, nullptr);
  }
  LocalFree(new_acl);
  LocalFree(security);
  return error;
}

struct Fixture {
  std::wstring name, directory, executable, dll;
  PSID sid = nullptr;
  bool profile_created = false;
  PROCESS_INFORMATION child{};
  ~Fixture() {
    if (child.hProcess) {
      if (WaitForSingleObject(child.hProcess, 0) == WAIT_TIMEOUT) {
        TerminateProcess(child.hProcess, 99);
        WaitForSingleObject(child.hProcess, 5000);
      }
      CloseHandle(child.hProcess);
    }
    if (child.hThread) CloseHandle(child.hThread);
    if (profile_created) {
      const HRESULT result = DeleteAppContainerProfile(name.c_str());
      if (FAILED(result)) fprintf(stderr, "profile cleanup: 0x%08lx\n", result);
    }
    if (sid) FreeSid(sid);
    if (!dll.empty()) DeleteFileW(dll.c_str());
    if (!executable.empty()) DeleteFileW(executable.c_str());
    if (!directory.empty()) RemoveDirectoryW(directory.c_str());
  }
};

int Parent(const wchar_t* source_dll) {
  Fixture fixture;
  fixture.name = L"KeyTao.Smoke." + std::to_wstring(GetCurrentProcessId()) +
                 L"." + std::to_wstring(GetTickCount64());
  wchar_t temporary[MAX_PATH], self[MAX_PATH];
  if (!GetTempPathW(MAX_PATH, temporary) ||
      !GetModuleFileNameW(nullptr, self, MAX_PATH)) return 1;
  fixture.directory = std::wstring(temporary) + fixture.name;
  if (!CreateDirectoryW(fixture.directory.c_str(), nullptr)) return 2;
  fixture.executable = fixture.directory + L"\\host.exe";
  fixture.dll = fixture.directory + L"\\keytao_windows_ime.dll";
  if (!CopyFileW(self, fixture.executable.c_str(), TRUE) ||
      !CopyFileW(source_dll, fixture.dll.c_str(), TRUE)) return 3;

  const HRESULT created = CreateAppContainerProfile(fixture.name.c_str(),
      L"KeyTao isolated smoke test", L"Temporary factory-load test", nullptr, 0,
      &fixture.sid);
  if (FAILED(created)) {
    fprintf(stderr, "CreateAppContainerProfile: 0x%08lx\n", created);
    return 4;
  }
  fixture.profile_created = true;
  // The exact same RX grant as the installer, only on our own copied fixture.
  PSID packages = nullptr;
  if (!ConvertStringSidToSidW(L"S-1-15-2-1", &packages)) return 5;
  DWORD acl_error = ERROR_SUCCESS;
  for (const auto& path : {fixture.directory, fixture.executable, fixture.dll}) {
    const DWORD result = GrantReadExecute(path, packages);
    if (result != ERROR_SUCCESS) acl_error = result;
  }
  LocalFree(packages);
  if (acl_error != ERROR_SUCCESS) {
    fprintf(stderr, "fixture RX grant: %lu\n", acl_error);
    return 5;
  }

  SIZE_T bytes = 0;
  InitializeProcThreadAttributeList(nullptr, 1, 0, &bytes);
  std::vector<unsigned char> storage(bytes);
  auto attributes = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage.data());
  if (!InitializeProcThreadAttributeList(attributes, 1, 0, &bytes)) return 6;
  SECURITY_CAPABILITIES capabilities{};
  capabilities.AppContainerSid = fixture.sid;
  const BOOL updated = UpdateProcThreadAttribute(attributes, 0,
      PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &capabilities,
      sizeof(capabilities), nullptr, nullptr);
  if (!updated) {
    DeleteProcThreadAttributeList(attributes);
    return 7;
  }
  STARTUPINFOEXW startup{};
  startup.StartupInfo.cb = sizeof(startup);
  startup.lpAttributeList = attributes;
  wchar_t system[MAX_PATH];
  GetSystemDirectoryW(system, MAX_PATH);
  std::wstring command = L"\"" + fixture.executable + L"\" --child \"" +
                         fixture.dll + L"\"";
  const BOOL launched = CreateProcessW(fixture.executable.c_str(), command.data(),
      nullptr, nullptr, FALSE, EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
      nullptr, system, &startup.StartupInfo, &fixture.child);
  const DWORD launch_error = GetLastError();
  DeleteProcThreadAttributeList(attributes);
  if (!launched) {
    fprintf(stderr, "CreateProcess(AppContainer): %lu\n", launch_error);
    return 8;
  }
  if (WaitForSingleObject(fixture.child.hProcess, 20000) != WAIT_OBJECT_0) {
    fprintf(stderr, "AppContainer child timed out\n");
    return 9;
  }
  DWORD exit_code = 0;
  if (!GetExitCodeProcess(fixture.child.hProcess, &exit_code)) return 9;
  printf("AppContainer factory/private-cache smoke: exit=%lu (0x%08lx)\n",
         exit_code, exit_code);
  return exit_code == 0 ? 0 : 1;
}
} // namespace

int wmain(int argc, wchar_t** argv) {
  if (argc == 3 && wcscmp(argv[1], L"--child") == 0) return Child(argv[2]);
  if (argc != 2) {
    fprintf(stderr, "Usage: appcontainer-load-test.exe <absolute-TIP-DLL>\n");
    return 1;
  }
  return Parent(argv[1]);
}
