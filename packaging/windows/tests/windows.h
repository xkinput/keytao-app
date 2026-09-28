// Minimal Win32 seam for host-only single-instance state-machine tests.
// This header is never included by the Windows runner build.
#pragma once
#include <cstddef>
#include <cstdint>

using BOOL = int;
using DWORD = unsigned long;
using ULONGLONG = unsigned long long;
using ULONG_PTR = std::uintptr_t;
using DWORD_PTR = std::uintptr_t;
using UINT = unsigned int;
using LPARAM = std::intptr_t;
using HANDLE = void*;
using HWND = void*;
struct COPYDATASTRUCT { ULONG_PTR dwData; DWORD cbData; void* lpData; };
constexpr BOOL TRUE = 1;
constexpr DWORD ERROR_ALREADY_EXISTS = 183;
constexpr DWORD WAIT_OBJECT_0 = 0;
constexpr DWORD WAIT_ABANDONED = 128;
constexpr DWORD WAIT_TIMEOUT = 258;
constexpr UINT WM_COPYDATA = 74;
constexpr UINT WM_APP = 0x8000;
constexpr UINT SMTO_ABORTIFHUNG = 2;
constexpr UINT SMTO_BLOCK = 1;
HANDLE CreateMutexW(void*, BOOL, const wchar_t*);
BOOL ReleaseMutex(HANDLE);
BOOL CloseHandle(HANDLE);
DWORD GetLastError();
ULONGLONG GetTickCount64();
DWORD WaitForSingleObject(HANDLE, DWORD);
HWND FindWindowW(const wchar_t*, const wchar_t*);
HANDLE GetPropW(HWND, const wchar_t*);
DWORD GetWindowThreadProcessId(HWND, DWORD*);
BOOL AllowSetForegroundWindow(DWORD);
wchar_t* GetCommandLineW();
BOOL SendMessageTimeoutW(HWND, UINT, std::uintptr_t, LPARAM, UINT, UINT, DWORD_PTR*);
void Sleep(DWORD);
