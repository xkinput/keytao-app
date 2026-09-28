#include "single_instance.h"

#include <cwchar>

SingleInstance::~SingleInstance() {
  if (owns_mutex_) ReleaseMutex(mutex_);
  if (mutex_) CloseHandle(mutex_);
}

SingleInstance::Result SingleInstance::AcquireOrForward() {
  mutex_ = CreateMutexW(nullptr, TRUE, L"Local\\ink.rea.keytao-app.Flutter");
  if (!mutex_) return Result::failed;
  if (GetLastError() != ERROR_ALREADY_EXISTS) {
    owns_mutex_ = true;
    return Result::primary;
  }

  // A concurrent launch can arrive before the primary creates its window.
  const ULONGLONG deadline = GetTickCount64() + 15000;
  while (GetTickCount64() < deadline) {
    const DWORD wait = WaitForSingleObject(mutex_, 0);
    if (wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED) {
      owns_mutex_ = true;
      return Result::primary;
    }
    HWND window = FindWindowW(kKeyTaoWindowClass, nullptr);
    if (window && GetPropW(window, kKeyTaoWindowReady)) {
      DWORD process_id = 0;
      GetWindowThreadProcessId(window, &process_id);
      AllowSetForegroundWindow(process_id);
      const wchar_t* command_line = GetCommandLineW();
      COPYDATASTRUCT data{};
      data.dwData = kKeyTaoArgsMessage;
      data.cbData = static_cast<DWORD>((wcslen(command_line) + 1) * sizeof(wchar_t));
      data.lpData = const_cast<wchar_t*>(command_line);
      DWORD_PTR accepted = 0;
      // Never retry a timed-out send: the first delivery may already be queued.
      return SendMessageTimeoutW(window, WM_COPYDATA, 0,
                                reinterpret_cast<LPARAM>(&data),
                                SMTO_ABORTIFHUNG | SMTO_BLOCK, 5000, &accepted) &&
                     accepted == TRUE
                 ? Result::forwarded
                 : Result::failed;
    }
    Sleep(50);
  }
  return Result::failed;
}
