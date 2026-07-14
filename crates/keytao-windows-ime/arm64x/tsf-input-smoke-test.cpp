#define UNICODE
#define _UNICODE

#include <msctf.h>
#include <oleauto.h>
#include <stdio.h>
#include <windows.h>

namespace {

const CLSID kTextService = {0x4A5C6D7E,
                            0x8F90,
                            0x1A2B,
                            {0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B, 0x9C, 0x0D}};
const GUID kProfile = {0x1B2C3D4E,
                       0x5F60,
                       0x7A8B,
                       {0x9C, 0x0D, 0x1E, 0x2F, 0x3A, 0x4B, 0x5C, 0x6D}};
const GUID kInputModeItem = {
    0xB35F6C5B,
    0xF641,
    0x42E4,
    {0x89, 0x74, 0x95, 0xFF, 0x7E, 0x1F, 0x38, 0x5C}};
constexpr LANGID kLanguage = 0x0804;

template <typename T> void Release(T *&value) {
  if (value != nullptr) {
    value->Release();
    value = nullptr;
  }
}

int Fail(const char *step, HRESULT result = E_FAIL) {
  fprintf(stderr, "%s failed: 0x%08lx\n", step,
          static_cast<unsigned long>(result));
  return 1;
}

void PumpFor(DWORD duration_ms) {
  const ULONGLONG deadline = GetTickCount64() + duration_ms;
  do {
    MSG message{};
    while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
      TranslateMessage(&message);
      DispatchMessageW(&message);
    }
    Sleep(10);
  } while (GetTickCount64() < deadline);
}

bool SendVirtualKey(WORD key) {
  INPUT input[2]{};
  input[0].type = INPUT_KEYBOARD;
  input[0].ki.wVk = key;
  input[1] = input[0];
  input[1].ki.dwFlags = KEYEVENTF_KEYUP;
  return SendInput(2, input, sizeof(INPUT)) == 2;
}

HRESULT NormalizeChineseMode(ITfThreadMgr *thread_manager) {
  ITfLangBarItemMgr *manager = nullptr;
  ITfLangBarItem *item = nullptr;
  ITfLangBarItemButton *button = nullptr;
  BSTR text = nullptr;
  HRESULT result = thread_manager->QueryInterface(IID_PPV_ARGS(&manager));
  if (FAILED(result)) {
    goto cleanup;
  }
  for (int attempt = 0; attempt < 40; ++attempt) {
    result = manager->GetItem(kInputModeItem, &item);
    if (SUCCEEDED(result) && item != nullptr) {
      break;
    }
    PumpFor(50);
  }
  if (FAILED(result) || item == nullptr) {
    goto cleanup;
  }
  result = item->QueryInterface(IID_PPV_ARGS(&button));
  if (FAILED(result)) {
    goto cleanup;
  }
  result = button->GetText(&text);
  if (FAILED(result) || text == nullptr || SysStringLen(text) != 1) {
    goto cleanup;
  }
  if (text[0] == 0x82F1) {
    POINT point{};
    RECT area{};
    result = button->OnClick(TF_LBI_CLK_LEFT, point, &area);
  } else if (text[0] != 0x4E2D) {
    result = E_FAIL;
  }

cleanup:
  if (text != nullptr) {
    SysFreeString(text);
  }
  Release(button);
  Release(item);
  Release(manager);
  return result;
}

} // namespace

int wmain(int argc, wchar_t **argv) {
  if (argc != 2) {
    fwprintf(stderr, L"usage: %ls <expected-target.dll>\n", argv[0]);
    return 2;
  }

  HRESULT result = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (FAILED(result)) {
    return Fail("CoInitializeEx", result);
  }

  ITfThreadMgr *thread_manager = nullptr;
  ITfInputProcessorProfiles *profiles = nullptr;
  ITfInputProcessorProfileMgr *profile_manager = nullptr;
  TfClientId client_id = TF_CLIENTID_NULL;
  HWND edit = nullptr;
  int exit_code = 1;

  result = CoCreateInstance(CLSID_TF_ThreadMgr, nullptr, CLSCTX_INPROC_SERVER,
                            IID_PPV_ARGS(&thread_manager));
  if (FAILED(result)) {
    Fail("CoCreateInstance(CLSID_TF_ThreadMgr)", result);
    goto cleanup;
  }
  result = thread_manager->Activate(&client_id);
  if (FAILED(result)) {
    Fail("ITfThreadMgr::Activate", result);
    goto cleanup;
  }

  edit = CreateWindowExW(WS_EX_CLIENTEDGE, L"EDIT", L"",
                         WS_OVERLAPPEDWINDOW | WS_VISIBLE | ES_LEFT |
                             ES_AUTOHSCROLL,
                         CW_USEDEFAULT, CW_USEDEFAULT, 640, 160, nullptr,
                         nullptr, GetModuleHandleW(nullptr), nullptr);
  if (edit == nullptr) {
    Fail("CreateWindowExW(EDIT)", HRESULT_FROM_WIN32(GetLastError()));
    goto cleanup;
  }
  ShowWindow(edit, SW_SHOW);
  UpdateWindow(edit);
  SetFocus(edit);

  result = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr,
                            CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&profiles));
  if (FAILED(result)) {
    Fail("CoCreateInstance(CLSID_TF_InputProcessorProfiles)", result);
    goto cleanup;
  }
  result = profiles->QueryInterface(IID_PPV_ARGS(&profile_manager));
  if (FAILED(result)) {
    Fail("ITfInputProcessorProfileMgr", result);
    goto cleanup;
  }
  result = profile_manager->ActivateProfile(
      TF_PROFILETYPE_INPUTPROCESSOR, kLanguage, kTextService, kProfile,
      nullptr, TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE);
  if (FAILED(result)) {
    Fail("ITfInputProcessorProfileMgr::ActivateProfile", result);
    goto cleanup;
  }
  result = profiles->ActivateLanguageProfile(kTextService, kLanguage, kProfile);
  if (FAILED(result)) {
    Fail("ITfInputProcessorProfiles::ActivateLanguageProfile", result);
    goto cleanup;
  }
  PumpFor(250);

  if (GetModuleHandleW(argv[1]) == nullptr) {
    fwprintf(stderr, L"Expected text service was not loaded: %ls\n", argv[1]);
    goto cleanup;
  }
  result = NormalizeChineseMode(thread_manager);
  if (FAILED(result)) {
    Fail("NormalizeChineseMode", result);
    goto cleanup;
  }

  // The deployed KeyTao dictionary contains a single-character entry for
  // "bbaa". A committed result must therefore contain non-ASCII text.
  PumpFor(1500);
  const WORD keys[] = {'B', 'B', 'A', 'A', VK_SPACE};
  for (WORD key : keys) {
    if (!SendVirtualKey(key)) {
      Fail("SendInput", HRESULT_FROM_WIN32(GetLastError()));
      goto cleanup;
    }
    PumpFor(100);
  }
  PumpFor(500);

  wchar_t value[256]{};
  const int length = GetWindowTextW(edit, value, 256);
  bool has_non_ascii = false;
  for (int index = 0; index < length; ++index) {
    has_non_ascii = has_non_ascii || value[index] > 0x7F;
  }
  if (length <= 0 || !has_non_ascii) {
    fwprintf(stderr, L"KeyTao did not commit Chinese text (length=%d, text='%ls')\n",
             length, value);
    goto cleanup;
  }

  wprintf(L"KeyTao committed Chinese text through %ls (U+%04X)\n", argv[1],
          static_cast<unsigned int>(value[0]));
  exit_code = 0;

cleanup:
  if (edit != nullptr) {
    DestroyWindow(edit);
  }
  Release(profile_manager);
  Release(profiles);
  if (thread_manager != nullptr && client_id != TF_CLIENTID_NULL) {
    thread_manager->Deactivate();
  }
  Release(thread_manager);
  CoUninitialize();
  return exit_code;
}
