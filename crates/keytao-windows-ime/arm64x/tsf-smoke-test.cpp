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

int Fail(const char *step, HRESULT result) {
  fprintf(stderr, "%s failed: 0x%08lx\n", step,
          static_cast<unsigned long>(result));
  return 1;
}

void PumpMessages() {
  MSG message{};
  while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
    TranslateMessage(&message);
    DispatchMessageW(&message);
  }
}

HRESULT GetLanguageBarItem(ITfLangBarItemMgr *manager,
                           ITfLangBarItem **item) {
  HRESULT result = E_FAIL;
  for (int attempt = 0; attempt < 20; ++attempt) {
    result = manager->GetItem(kInputModeItem, item);
    if (SUCCEEDED(result) && *item != nullptr) {
      return S_OK;
    }
    PumpMessages();
    Sleep(50);
  }
  return result;
}

} // namespace

int wmain(int argc, wchar_t **argv) {
  if (argc < 2 || argc > 3) {
    fwprintf(stderr, L"usage: %ls <expected-target.dll> [--enable]\n",
             argv[0]);
    return 2;
  }
  const bool attempt_enable = argc == 3 && wcscmp(argv[2], L"--enable") == 0;

  HRESULT result = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (FAILED(result)) {
    return Fail("CoInitializeEx", result);
  }

  ITfThreadMgr *thread_manager = nullptr;
  ITfDocumentMgr *document_manager = nullptr;
  ITfContext *context = nullptr;
  ITfInputProcessorProfiles *profiles = nullptr;
  ITfInputProcessorProfileMgr *profile_manager = nullptr;
  ITfLangBarItemMgr *language_bar_manager = nullptr;
  ITfLangBarItem *language_bar_item = nullptr;
  ITfLangBarItemButton *language_bar_button = nullptr;
  TfClientId client_id = TF_CLIENTID_NULL;
  TfEditCookie edit_cookie = TF_INVALID_COOKIE;
  BOOL enabled = FALSE;
  LANGID active_language = 0;
  GUID active_profile{};
  BSTR mode_text = nullptr;
  HICON mode_icon = nullptr;
  POINT point{};
  RECT area{};
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

  result = thread_manager->CreateDocumentMgr(&document_manager);
  if (FAILED(result)) {
    Fail("ITfThreadMgr::CreateDocumentMgr", result);
    goto cleanup;
  }
  result = document_manager->CreateContext(client_id, 0, nullptr, &context,
                                           &edit_cookie);
  if (FAILED(result)) {
    Fail("ITfDocumentMgr::CreateContext", result);
    goto cleanup;
  }
  result = document_manager->Push(context);
  if (FAILED(result)) {
    Fail("ITfDocumentMgr::Push", result);
    goto cleanup;
  }
  result = thread_manager->SetFocus(document_manager);
  if (FAILED(result)) {
    Fail("ITfThreadMgr::SetFocus", result);
    goto cleanup;
  }

  result = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr,
                            CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&profiles));
  if (FAILED(result)) {
    Fail("CoCreateInstance(CLSID_TF_InputProcessorProfiles)", result);
    goto cleanup;
  }

  result = profiles->IsEnabledLanguageProfile(kTextService, kLanguage, kProfile,
                                               &enabled);
  if (FAILED(result)) {
    Fail("ITfInputProcessorProfiles::IsEnabledLanguageProfile", result);
    goto cleanup;
  }
  printf("Profile enabled: %s\n", enabled ? "true" : "false");
  if (!enabled && attempt_enable) {
    result = profiles->EnableLanguageProfile(kTextService, kLanguage, kProfile,
                                             TRUE);
    printf("EnableLanguageProfile: 0x%08lx\n",
           static_cast<unsigned long>(result));
    if (SUCCEEDED(result)) {
      result = profiles->IsEnabledLanguageProfile(kTextService, kLanguage,
                                                   kProfile, &enabled);
    }
    printf("Profile enabled after legacy enable: %s\n",
           enabled ? "true" : "false");
  }
  if (attempt_enable) {
    result = profiles->EnableLanguageProfileByDefault(
        kTextService, kLanguage, kProfile, TRUE);
    printf("EnableLanguageProfileByDefault: 0x%08lx\n",
           static_cast<unsigned long>(result));
    if (SUCCEEDED(result)) {
      result = profiles->IsEnabledLanguageProfile(kTextService, kLanguage,
                                                   kProfile, &enabled);
    }
    printf("Profile enabled after default enable: %s\n",
           enabled ? "true" : "false");
  }
  if (!enabled && attempt_enable) {
    result = profiles->QueryInterface(IID_PPV_ARGS(&profile_manager));
    if (SUCCEEDED(result)) {
      result = profile_manager->ActivateProfile(
          TF_PROFILETYPE_INPUTPROCESSOR, kLanguage, kTextService, kProfile,
          nullptr,
          TF_IPPMF_ENABLEPROFILE | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE);
      printf("ActivateProfile(TF_IPPMF_ENABLEPROFILE): 0x%08lx\n",
             static_cast<unsigned long>(result));
      if (SUCCEEDED(result)) {
        result = profiles->IsEnabledLanguageProfile(kTextService, kLanguage,
                                                     kProfile, &enabled);
      }
      printf("Profile enabled after modern enable: %s\n",
             enabled ? "true" : "false");
    }
  }
  if (!enabled) {
    fprintf(stderr, "KeyTao profile is registered but disabled\n");
    goto cleanup;
  }

  if (profile_manager == nullptr) {
    result = profiles->QueryInterface(IID_PPV_ARGS(&profile_manager));
    if (FAILED(result)) {
      Fail("ITfInputProcessorProfileMgr", result);
      goto cleanup;
    }
  }
  result = profile_manager->ActivateProfile(
      TF_PROFILETYPE_INPUTPROCESSOR, kLanguage, kTextService, kProfile,
      nullptr, TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE);
  printf("ActivateProfile(TF_IPPMF_FORSESSION): 0x%08lx\n",
         static_cast<unsigned long>(result));
  if (FAILED(result)) {
    goto cleanup;
  }

  result =
      profiles->ActivateLanguageProfile(kTextService, kLanguage, kProfile);
  if (FAILED(result)) {
    Fail("ITfInputProcessorProfiles::ActivateLanguageProfile", result);
    goto cleanup;
  }
  PumpMessages();

  result = profiles->GetActiveLanguageProfile(kTextService, &active_language,
                                               &active_profile);
  if (FAILED(result) || active_language != kLanguage ||
      !IsEqualGUID(active_profile, kProfile)) {
    Fail("ITfInputProcessorProfiles::GetActiveLanguageProfile",
         FAILED(result) ? result : E_FAIL);
    goto cleanup;
  }

  if (GetModuleHandleW(argv[1]) == nullptr) {
    fwprintf(stderr, L"Expected text service was not loaded: %ls\n", argv[1]);
    goto cleanup;
  }

  result = thread_manager->QueryInterface(IID_PPV_ARGS(&language_bar_manager));
  if (FAILED(result)) {
    result = CoCreateInstance(CLSID_TF_LangBarItemMgr, nullptr,
                              CLSCTX_INPROC_SERVER,
                              IID_PPV_ARGS(&language_bar_manager));
  }
  if (FAILED(result)) {
    Fail("ITfLangBarItemMgr", result);
    goto cleanup;
  }

  result = GetLanguageBarItem(language_bar_manager, &language_bar_item);
  if (FAILED(result)) {
    Fail("ITfLangBarItemMgr::GetItem", result);
    goto cleanup;
  }

  result = language_bar_item->QueryInterface(
      IID_PPV_ARGS(&language_bar_button));
  if (FAILED(result)) {
    Fail("ITfLangBarItemButton", result);
    goto cleanup;
  }

  DWORD language_bar_status = TF_LBI_STATUS_HIDDEN;
  result = language_bar_item->GetStatus(&language_bar_status);
  if (FAILED(result) || (language_bar_status & TF_LBI_STATUS_HIDDEN) != 0) {
    Fail("ITfLangBarItem::GetStatus(visible)",
         FAILED(result) ? result : E_FAIL);
    goto cleanup;
  }

  result = language_bar_button->GetText(&mode_text);
  if (SUCCEEDED(result) && mode_text != nullptr &&
      SysStringLen(mode_text) == 1 && mode_text[0] == 0x82F1) {
    SysFreeString(mode_text);
    mode_text = nullptr;
    result = language_bar_button->OnClick(TF_LBI_CLK_LEFT, point, &area);
    if (FAILED(result)) {
      Fail("ITfLangBarItemButton::OnClick(normalize Chinese)", result);
      goto cleanup;
    }
    result = language_bar_button->GetText(&mode_text);
  }
  if (FAILED(result) || mode_text == nullptr || SysStringLen(mode_text) != 1 ||
      mode_text[0] != 0x4E2D) {
    if (SUCCEEDED(result) && mode_text != nullptr) {
      fwprintf(stderr,
               L"Unexpected initial input mode text: '%ls' (length=%u, "
               L"U+%04X U+%04X U+%04X)\n",
               mode_text, static_cast<unsigned int>(SysStringLen(mode_text)),
               static_cast<unsigned int>(mode_text[0]),
               static_cast<unsigned int>(mode_text[1]),
               static_cast<unsigned int>(mode_text[2]));
    }
    if (mode_text != nullptr) {
      SysFreeString(mode_text);
    }
    Fail("ITfLangBarItemButton::GetText(Chinese)",
         FAILED(result) ? result : E_FAIL);
    goto cleanup;
  }
  SysFreeString(mode_text);

  result = language_bar_button->GetIcon(&mode_icon);
  if (FAILED(result) || mode_icon == nullptr) {
    Fail("ITfLangBarItemButton::GetIcon", FAILED(result) ? result : E_FAIL);
    goto cleanup;
  }

  result = language_bar_button->OnClick(TF_LBI_CLK_LEFT, point, &area);
  if (FAILED(result)) {
    Fail("ITfLangBarItemButton::OnClick(English)", result);
    goto cleanup;
  }

  mode_text = nullptr;
  result = language_bar_button->GetText(&mode_text);
  if (FAILED(result) || mode_text == nullptr || SysStringLen(mode_text) != 1 ||
      mode_text[0] != 0x82F1) {
    if (mode_text != nullptr) {
      SysFreeString(mode_text);
    }
    Fail("ITfLangBarItemButton::GetText(English)",
         FAILED(result) ? result : E_FAIL);
    goto cleanup;
  }
  SysFreeString(mode_text);

  result = language_bar_button->OnClick(TF_LBI_CLK_LEFT, point, &area);
  if (FAILED(result)) {
    Fail("ITfLangBarItemButton::OnClick(Chinese)", result);
    goto cleanup;
  }

  wprintf(L"Activated KeyTao through %ls with a working Chinese/English item\n",
          argv[1]);
  exit_code = 0;

cleanup:
  Release(language_bar_button);
  Release(language_bar_item);
  Release(language_bar_manager);
  Release(profile_manager);
  Release(profiles);
  if (document_manager != nullptr) {
    document_manager->Pop(TF_POPF_ALL);
  }
  Release(context);
  Release(document_manager);
  if (thread_manager != nullptr && client_id != TF_CLIENTID_NULL) {
    thread_manager->Deactivate();
  }
  Release(thread_manager);
  CoUninitialize();
  return exit_code;
}
