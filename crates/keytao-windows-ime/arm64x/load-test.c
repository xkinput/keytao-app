#include <stdio.h>
#include <windows.h>

typedef HRESULT(WINAPI *DllCanUnloadNowFn)(void);

int wmain(int argc, wchar_t **argv) {
  static const char *const exports[] = {
      "DllGetClassObject",
      "DllCanUnloadNow",
      "DllRegisterServer",
      "DllUnregisterServer",
  };

  if (argc != 3) {
    fwprintf(stderr, L"usage: %ls <wrapper.dll> <expected-target.dll>\n",
             argv[0]);
    return 2;
  }

  HMODULE wrapper =
      LoadLibraryExW(argv[1], NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
  if (wrapper == NULL) {
    fwprintf(stderr, L"LoadLibraryExW failed: %lu\n", GetLastError());
    return 3;
  }

  FARPROC can_unload = NULL;
  for (size_t i = 0; i < ARRAYSIZE(exports); ++i) {
    FARPROC address = GetProcAddress(wrapper, exports[i]);
    if (address == NULL) {
      fprintf(stderr, "GetProcAddress(%s) failed: %lu\n", exports[i],
              GetLastError());
      FreeLibrary(wrapper);
      return 4;
    }
    if (i == 1) {
      can_unload = address;
    }
  }

  HRESULT unload_result = ((DllCanUnloadNowFn)can_unload)();
  if (unload_result != S_OK && unload_result != S_FALSE) {
    fprintf(stderr, "DllCanUnloadNow returned 0x%08lx\n",
            (unsigned long)unload_result);
    FreeLibrary(wrapper);
    return 5;
  }

  if (GetModuleHandleW(argv[2]) == NULL) {
    fwprintf(stderr, L"Expected target was not loaded: %ls\n", argv[2]);
    FreeLibrary(wrapper);
    return 6;
  }

  wprintf(L"Loaded %ls through %ls\n", argv[2], argv[1]);
  FreeLibrary(wrapper);
  return 0;
}
