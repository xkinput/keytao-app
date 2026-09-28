#include "single_instance.h"

#include <cassert>
#include <cwchar>
#include <iostream>

namespace {
struct Scenario {
  bool existing = false;
  bool create_fails = false;
  bool window_exists = true;
  bool send_succeeds = true;
  DWORD wait_result = WAIT_TIMEOUT;
  ULONGLONG ready_at = 0;
  ULONGLONG now = 0;
  int sends = 0;
  int releases = 0;
  int closes = 0;
  DWORD allowed_process = 0;
} state;
auto fake_handle = reinterpret_cast<HANDLE>(1);
wchar_t command_line[] = L"\"C:\\Program Files\\KeyTao\\keytao-app.exe\" keytao://ime/redeploy \"two words\" \"\" 中文";
}  // namespace

HANDLE CreateMutexW(void*, BOOL owner, const wchar_t* name) {
  assert(owner == TRUE);
  assert(wcscmp(name, L"Local\\ink.rea.keytao-app.Flutter") == 0);
  return state.create_fails ? nullptr : fake_handle;
}
BOOL ReleaseMutex(HANDLE) { ++state.releases; return TRUE; }
BOOL CloseHandle(HANDLE) { ++state.closes; return TRUE; }
DWORD GetLastError() { return state.existing ? ERROR_ALREADY_EXISTS : 0; }
ULONGLONG GetTickCount64() { return state.now; }
DWORD WaitForSingleObject(HANDLE, DWORD timeout) {
  assert(timeout == 0);
  return state.wait_result;
}
HWND FindWindowW(const wchar_t* name, const wchar_t*) {
  assert(wcscmp(name, kKeyTaoWindowClass) == 0);
  return state.window_exists ? fake_handle : nullptr;
}
HANDLE GetPropW(HWND, const wchar_t* name) {
  assert(wcscmp(name, kKeyTaoWindowReady) == 0);
  return state.now >= state.ready_at ? fake_handle : nullptr;
}
DWORD GetWindowThreadProcessId(HWND, DWORD* process) { *process = 42; return 1; }
BOOL AllowSetForegroundWindow(DWORD process) { state.allowed_process = process; return TRUE; }
wchar_t* GetCommandLineW() { return command_line; }
BOOL SendMessageTimeoutW(HWND, UINT message, std::uintptr_t, LPARAM payload,
                        UINT flags, UINT timeout, DWORD_PTR* accepted) {
  ++state.sends;
  assert(message == WM_COPYDATA && timeout == 5000);
  assert(flags == (SMTO_ABORTIFHUNG | SMTO_BLOCK));
  auto* data = reinterpret_cast<COPYDATASTRUCT*>(payload);
  assert(data->dwData == kKeyTaoArgsMessage);
  assert(data->cbData == sizeof(command_line));
  assert(wcscmp(static_cast<wchar_t*>(data->lpData), command_line) == 0);
  *accepted = TRUE;
  return state.send_succeeds;
}
void Sleep(DWORD duration) { state.now += duration; }

int main() {
  using Result = SingleInstance::Result;
  {
    SingleInstance primary;
    assert(primary.AcquireOrForward() == Result::primary);
  }
  assert(state.releases == 1 && state.closes == 1 && state.sends == 0);
  state = {};
  state.existing = true;
  state.ready_at = 250;
  {
    SingleInstance secondary;
    assert(secondary.AcquireOrForward() == Result::forwarded);
  }
  assert(state.now == 250 && state.sends == 1 && state.allowed_process == 42);
  assert(state.releases == 0 && state.closes == 1);
  state = {};
  state.existing = true;
  state.wait_result = WAIT_ABANDONED;
  {
    SingleInstance recovered;
    assert(recovered.AcquireOrForward() == Result::primary);
  }
  assert(state.releases == 1 && state.sends == 0);
  state = {};
  state.existing = true;
  state.send_succeeds = false;
  {
    SingleInstance timeout;
    assert(timeout.AcquireOrForward() == Result::failed);
  }
  assert(state.sends == 1 && state.releases == 0);
  state = {};
  state.existing = true;
  state.window_exists = false;
  {
    SingleInstance unavailable;
    assert(unavailable.AcquireOrForward() == Result::failed);
  }
  assert(state.now == 15000 && state.sends == 0);
  state = {};
  state.create_fails = true;
  {
    SingleInstance denied;
    assert(denied.AcquireOrForward() == Result::failed);
  }
  assert(state.closes == 0 && state.releases == 0);
  std::cout << "PASS: primary, startup race, abandoned owner, send timeout, missing window, mutex failure\n";
}
