#include "flutter_window.h"

#include <shellapi.h>
#include <shobjidl.h>
#include <wrl/client.h>
#include <flutter/standard_method_codec.h>

#include <cwchar>
#include <optional>

#include "flutter/generated_plugin_registrant.h"
#include "single_instance.h"
#include "utils.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());
  args_channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "keytao/windows",
      &flutter::StandardMethodCodec::GetInstance());

  flutter::MethodChannel<flutter::EncodableValue> window_channel(
      flutter_controller_->engine()->messenger(), "ink.rea.keytao/window",
      &flutter::StandardMethodCodec::GetInstance());
  window_channel.SetMethodCallHandler(
      [owner = GetHandle()](const auto& call, auto result) {
        if (call.method_name() != "pickDirectory") {
          result->NotImplemented();
          return;
        }
        Microsoft::WRL::ComPtr<IFileOpenDialog> dialog;
        HRESULT hr = CoCreateInstance(CLSID_FileOpenDialog, nullptr,
                                      CLSCTX_INPROC_SERVER,
                                      IID_PPV_ARGS(dialog.GetAddressOf()));
        DWORD options = 0;
        if (SUCCEEDED(hr)) hr = dialog->GetOptions(&options);
        if (SUCCEEDED(hr)) {
          hr = dialog->SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        }
        if (SUCCEEDED(hr)) hr = dialog->Show(owner);
        if (hr == HRESULT_FROM_WIN32(ERROR_CANCELLED)) {
          result->Success();
          return;
        }
        Microsoft::WRL::ComPtr<IShellItem> item;
        if (SUCCEEDED(hr)) hr = dialog->GetResult(item.GetAddressOf());
        PWSTR path = nullptr;
        if (SUCCEEDED(hr)) hr = item->GetDisplayName(SIGDN_FILESYSPATH, &path);
        const std::string utf8 = SUCCEEDED(hr) ? Utf8FromUtf16(path) : "";
        CoTaskMemFree(path);
        if (FAILED(hr) || utf8.empty()) {
          result->Error("PICK_DIRECTORY_FAILED", "Cannot select a directory.");
          return;
        }
        result->Success(flutter::EncodableValue(utf8));
      });

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
    PostMessage(GetHandle(), kKeyTaoFirstFrame, 0, 0);
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();
  SetPropW(GetHandle(), kKeyTaoWindowReady, reinterpret_cast<HANDLE>(1));

  return true;
}

void FlutterWindow::OnDestroy() {
  RemovePropW(GetHandle(), kKeyTaoWindowReady);
  args_channel_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

bool FlutterWindow::ReceiveArgs(const COPYDATASTRUCT* data) {
  if (!flutter_controller_ || !args_channel_ || !data ||
      data->dwData != kKeyTaoArgsMessage || !data->lpData ||
      data->cbData < sizeof(wchar_t) || data->cbData > 32768 * sizeof(wchar_t) ||
      data->cbData % sizeof(wchar_t) != 0 || pending_args_.size() >= 64) {
    return false;
  }
  // Copy the payload before returning from WM_COPYDATA. Reject embedded NULs.
  const auto* text = static_cast<const wchar_t*>(data->lpData);
  const size_t length = data->cbData / sizeof(wchar_t);
  if (text[length - 1] != L'\0' || wcsnlen(text, length) != length - 1) return false;
  int count = 0;
  wchar_t** args = CommandLineToArgvW(text, &count);
  if (!args) return false;
  std::vector<std::string> values;
  for (int i = 1; i < count; ++i) values.push_back(Utf8FromUtf16(args[i]));
  LocalFree(args);
  pending_args_.push_back(std::move(values));
  ShowWindow(GetHandle(), IsIconic(GetHandle()) ? SW_RESTORE : SW_SHOW);
  SetForegroundWindow(GetHandle());
  SetFocus(flutter_controller_->view()->GetNativeWindow());
  DeliverPendingArgs();
  return true;
}

void FlutterWindow::DeliverPendingArgs() {
  if (!dart_ready_ || !args_channel_) return;
  while (!pending_args_.empty()) {
    flutter::EncodableList values;
    for (const auto& arg : pending_args_.front()) values.emplace_back(arg);
    pending_args_.pop_front();
    args_channel_->InvokeMethod("appArgs",
        std::make_unique<flutter::EncodableValue>(std::move(values)));
  }
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  if (message == WM_COPYDATA) {
    return ReceiveArgs(reinterpret_cast<const COPYDATASTRUCT*>(lparam)) ? TRUE : FALSE;
  }
  if (message == kKeyTaoFirstFrame) {
    dart_ready_ = true;
    DeliverPendingArgs();
    return 0;
  }
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
