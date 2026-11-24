#include "webview_core/webview_core_plugin_c_api.h"

#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <flutter/plugin_registrar_windows.h>
#include <flutter/standard_method_codec.h>
#include <flutter/texture_registrar.h>

#include <windows.h>
#include <commctrl.h>
#include <d3d11.h>
#include <dxgi.h>
#include <roapi.h>
#include <shlwapi.h>
#include <winstring.h>

#pragma warning(push, 3)
#include <DispatcherQueue.h>
#include <WebView2.h>
#include <windows.foundation.h>
#include <windows.graphics.capture.h>
#include <windows.graphics.directx.direct3d11.h>
#include <windows.ui.composition.h>
#include <wrl/client.h>
#include <wrl/event.h>
#pragma warning(pop)

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <utility>
#include <vector>

namespace {

using Microsoft::WRL::Callback;
using Microsoft::WRL::ComPtr;

constexpr size_t kMaxInlineNavigateToStringBytes = 1500000;

MIDL_INTERFACE("A9B3D012-3DF2-4EE3-B8D1-8695F457D3C1")
IDirect3DDxgiInterfaceAccessLocal : public IUnknown {
  virtual HRESULT STDMETHODCALLTYPE GetInterface(REFIID iid, void** p) = 0;
};

using CreateDispatcherQueueControllerFn =
    HRESULT(WINAPI*)(DispatcherQueueOptions,
                     ABI::Windows::System::IDispatcherQueueController**);
using CreateDirect3D11DeviceFromDXGIDeviceFn =
    HRESULT(WINAPI*)(IDXGIDevice*, IInspectable**);

std::wstring Utf8ToWide(const std::string& str) {
  if (str.empty()) {
    return std::wstring();
  }
  const int needed = MultiByteToWideChar(
      CP_UTF8, 0, str.data(), static_cast<int>(str.size()), nullptr, 0);
  if (needed <= 0) {
    return std::wstring();
  }
  std::wstring out(static_cast<size_t>(needed), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, str.data(), static_cast<int>(str.size()),
                      &out[0], needed);
  return out;
}

std::string WideToUtf8(const wchar_t* wstr) {
  if (wstr == nullptr || wstr[0] == L'\0') {
    return std::string();
  }
  const int len = static_cast<int>(wcslen(wstr));
  const int needed =
      WideCharToMultiByte(CP_UTF8, 0, wstr, len, nullptr, 0, nullptr, nullptr);
  if (needed <= 0) {
    return std::string();
  }
  std::string out(static_cast<size_t>(needed), '\0');
  WideCharToMultiByte(CP_UTF8, 0, wstr, len, &out[0], needed, nullptr, nullptr);
  return out;
}

std::string EscapeJsonString(const std::string& input) {
  std::string out;
  out.reserve(input.size() + 2);
  out.push_back('"');
  for (size_t i = 0; i < input.size(); ++i) {
    const unsigned char c = static_cast<unsigned char>(input[i]);
    switch (c) {
      case '"':
        out += "\\\"";
        break;
      case '\\':
        out += "\\\\";
        break;
      case '\n':
        out += "\\n";
        break;
      case '\r':
        out += "\\r";
        break;
      case '\t':
        out += "\\t";
        break;
      default:
        if (c < 0x20) {
          char buf[8];
          snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned int>(c));
          out += buf;
        } else {
          out.push_back(static_cast<char>(c));
        }
        break;
    }
  }
  out.push_back('"');
  return out;
}

const flutter::EncodableValue* LookupMap(const flutter::EncodableMap& map,
                                         const char* key) {
  const auto it = map.find(flutter::EncodableValue(key));
  return it != map.end() ? &it->second : nullptr;
}

bool GetIntArg(const flutter::EncodableMap& map,
               const char* key,
               int64_t* out) {
  const flutter::EncodableValue* v = LookupMap(map, key);
  if (v == nullptr) {
    return false;
  }
  if (std::holds_alternative<int32_t>(*v) ||
      std::holds_alternative<int64_t>(*v)) {
    *out = v->LongValue();
    return true;
  }
  return false;
}

double GetDoubleArg(const flutter::EncodableMap& map,
                    const char* key,
                    double fallback) {
  const flutter::EncodableValue* v = LookupMap(map, key);
  if (v == nullptr) {
    return fallback;
  }
  if (const auto* d = std::get_if<double>(v)) {
    return *d;
  }
  if (std::holds_alternative<int32_t>(*v) ||
      std::holds_alternative<int64_t>(*v)) {
    return static_cast<double>(v->LongValue());
  }
  return fallback;
}

bool GetBoolArg(const flutter::EncodableMap& map,
                const char* key,
                bool fallback) {
  const flutter::EncodableValue* v = LookupMap(map, key);
  if (v == nullptr) {
    return fallback;
  }
  if (const auto* b = std::get_if<bool>(v)) {
    return *b;
  }
  return fallback;
}

std::string GetStringArg(const flutter::EncodableMap& map,
                         const char* key,
                         bool* found = nullptr) {
  const flutter::EncodableValue* v = LookupMap(map, key);
  if (v == nullptr) {
    if (found != nullptr) {
      *found = false;
    }
    return std::string();
  }
  if (const auto* s = std::get_if<std::string>(v)) {
    if (found != nullptr) {
      *found = true;
    }
    return *s;
  }
  if (found != nullptr) {
    *found = false;
  }
  return std::string();
}

flutter::EncodableValue DecodeJsonScalar(const std::string& json) {
  if (json.empty() || json == "null" || json == "undefined") {
    return flutter::EncodableValue();
  }
  if (json == "true") {
    return flutter::EncodableValue(true);
  }
  if (json == "false") {
    return flutter::EncodableValue(false);
  }
  if (json.size() >= 2 && json.front() == '"' && json.back() == '"') {
    std::string unescaped;
    unescaped.reserve(json.size() - 2);
    for (size_t i = 1; i + 1 < json.size(); ++i) {
      const char ch = json[i];
      if (ch == '\\' && i + 2 < json.size()) {
        ++i;
        switch (json[i]) {
          case '"':
            unescaped.push_back('"');
            break;
          case '\\':
            unescaped.push_back('\\');
            break;
          case '/':
            unescaped.push_back('/');
            break;
          case 'n':
            unescaped.push_back('\n');
            break;
          case 'r':
            unescaped.push_back('\r');
            break;
          case 't':
            unescaped.push_back('\t');
            break;
          case 'b':
            unescaped.push_back('\b');
            break;
          case 'f':
            unescaped.push_back('\f');
            break;
          default:
            unescaped.push_back(json[i]);
            break;
        }
      } else {
        unescaped.push_back(ch);
      }
    }
    return flutter::EncodableValue(unescaped);
  }
  char* end_int = nullptr;
  const long long iv = std::strtoll(json.c_str(), &end_int, 10);
  if (end_int != nullptr && *end_int == '\0' && end_int != json.c_str()) {
    return flutter::EncodableValue(static_cast<int64_t>(iv));
  }
  char* end_dbl = nullptr;
  const double dv = std::strtod(json.c_str(), &end_dbl);
  if (end_dbl != nullptr && *end_dbl == '\0' && end_dbl != json.c_str() &&
      std::isfinite(dv)) {
    return flutter::EncodableValue(dv);
  }
  return flutter::EncodableValue(json);
}

std::string InjectBaseTagIfNeeded(const std::string& html,
                                  const std::string& base_url) {
  if (base_url.empty()) {
    return html;
  }
  std::string lower;
  lower.reserve(html.size());
  for (size_t i = 0; i < html.size(); ++i) {
    const unsigned char c = static_cast<unsigned char>(html[i]);
    lower.push_back(
        static_cast<char>((c >= 'A' && c <= 'Z') ? (c - 'a' + 'A') : c));
  }
  if (lower.find("<base ") != std::string::npos ||
      lower.find("<base>") != std::string::npos) {
    return html;
  }
  std::string escaped_base;
  escaped_base.reserve(base_url.size());
  for (size_t i = 0; i < base_url.size(); ++i) {
    switch (base_url[i]) {
      case '&':
        escaped_base += "&amp;";
        break;
      case '"':
        escaped_base += "&quot;";
        break;
      case '<':
        escaped_base += "&lt;";
        break;
      case '>':
        escaped_base += "&gt;";
        break;
      default:
        escaped_base.push_back(base_url[i]);
        break;
    }
  }
  const std::string base_tag = "<base href=\"" + escaped_base + "\">";
  const size_t head_pos = lower.find("<head");
  if (head_pos != std::string::npos) {
    const size_t gt = html.find('>', head_pos);
    if (gt != std::string::npos) {
      return html.substr(0, gt + 1) + base_tag + html.substr(gt + 1);
    }
  }
  const size_t html_pos = lower.find("<html");
  if (html_pos != std::string::npos) {
    const size_t gt = html.find('>', html_pos);
    if (gt != std::string::npos) {
      return html.substr(0, gt + 1) + "<head>" + base_tag + "</head>" +
             html.substr(gt + 1);
    }
  }
  const size_t doctype_pos = lower.find("<!doctype");
  if (doctype_pos != std::string::npos) {
    const size_t gt = html.find('>', doctype_pos);
    if (gt != std::string::npos) {
      return html.substr(0, gt + 1) + "<head>" + base_tag + "</head>" +
             html.substr(gt + 1);
    }
  }
  return base_tag + html;
}

std::wstring ResolveWritableUserDataFolder() {
  wchar_t local_app_data[MAX_PATH] = {};
  const DWORD env_len = GetEnvironmentVariableW(
      L"LOCALAPPDATA", local_app_data, MAX_PATH);
  std::wstring base = env_len > 0 && env_len < MAX_PATH
                          ? std::wstring(local_app_data, env_len)
                          : std::wstring(L".");

  wchar_t module_path[MAX_PATH] = {};
  GetModuleFileNameW(nullptr, module_path, MAX_PATH);
  const wchar_t* file_name = PathFindFileNameW(module_path);
  std::wstring exe_name =
      (file_name != nullptr && file_name[0] != L'\0') ? file_name : L"app";

  std::wstring root_dir = base + L"\\webview_core";
  CreateDirectoryW(root_dir.c_str(), nullptr);
  std::wstring app_dir = root_dir + L"\\" + exe_name;
  CreateDirectoryW(app_dir.c_str(), nullptr);
  return app_dir;
}

constexpr UINT kWmDrainUiTasks = WM_APP + 0x57;
constexpr const wchar_t kUiTaskWindowClassName[] =
    L"WebviewCoreUiTaskRunner";

struct UiTaskQueue {
  std::mutex mutex;
  std::vector<std::function<void()>> tasks;
  HWND hwnd = nullptr;
  bool closed = false;
};

LRESULT CALLBACK UiTaskRunnerWndProc(HWND hwnd,
                                     UINT msg,
                                     WPARAM wp,
                                     LPARAM lp) {
  if (msg == WM_NCCREATE) {
    const auto* cs = reinterpret_cast<const CREATESTRUCTW*>(lp);
    if (cs != nullptr && cs->lpCreateParams != nullptr) {
      ::SetWindowLongPtrW(hwnd, GWLP_USERDATA,
                          reinterpret_cast<LONG_PTR>(cs->lpCreateParams));
    }
    return TRUE;
  }
  if (msg == kWmDrainUiTasks) {
    auto* queue = reinterpret_cast<UiTaskQueue*>(
        ::GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (queue != nullptr) {
      std::vector<std::function<void()>> pending;
      {
        std::lock_guard<std::mutex> lock(queue->mutex);
        if (!queue->closed) {
          pending.swap(queue->tasks);
        }
      }
      for (auto& task : pending) {
        if (task) {
          task();
        }
      }
    }
    return 0;
  }
  if (msg == WM_NCDESTROY) {
    ::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    return 0;
  }
  return ::DefWindowProcW(hwnd, msg, wp, lp);
}

HWND CreateUiTaskRunnerWindow(UiTaskQueue* queue) {
  if (queue == nullptr) {
    return nullptr;
  }
  static bool registered = false;
  const HINSTANCE instance = ::GetModuleHandleW(nullptr);
  if (!registered) {
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = UiTaskRunnerWndProc;
    wc.hInstance = instance;
    wc.lpszClassName = kUiTaskWindowClassName;
    if (::RegisterClassExW(&wc) != 0 ||
        ::GetLastError() == ERROR_CLASS_ALREADY_EXISTS) {
      registered = true;
    }
  }
  if (!registered) {
    return nullptr;
  }
  return ::CreateWindowExW(0, kUiTaskWindowClassName, L"", 0, 0, 0, 0, 0,
                           HWND_MESSAGE, nullptr, instance, queue);
}

class CompositionGraphicsRuntime {
 public:
  ~CompositionGraphicsRuntime() {
    HWND hwnd_to_destroy = nullptr;
    if (task_queue_) {
      std::lock_guard<std::mutex> lock(task_queue_->mutex);
      task_queue_->closed = true;
      task_queue_->tasks.clear();
      hwnd_to_destroy = task_queue_->hwnd;
      task_queue_->hwnd = nullptr;
    }
    if (hwnd_to_destroy != nullptr && ::IsWindow(hwnd_to_destroy)) {
      ::SetWindowLongPtrW(hwnd_to_destroy, GWLP_USERDATA, 0);
      ::DestroyWindow(hwnd_to_destroy);
    }
  }

  bool EnsureInitialized() {
    if (initialized_) {
      return valid_;
    }
    initialized_ = true;

    if (task_queue_ && task_queue_->hwnd == nullptr) {
      task_queue_->hwnd = CreateUiTaskRunnerWindow(task_queue_.get());
    }
    if (!task_queue_ || task_queue_->hwnd == nullptr) {
      return false;
    }

    RoInitialize(RO_INIT_SINGLETHREADED);

    // Ensure a thread DispatcherQueue exists for Windows.UI.Composition.
    // When another plugin (e.g. window_manager / cnativeapi) already created
    // one on the UI thread, CreateDispatcherQueueController returns an error
    // and Compositor activation uses the existing thread DispatcherQueue.
    HMODULE core_messaging = LoadLibraryW(L"CoreMessaging.dll");
    if (core_messaging != nullptr) {
      auto create_dq = reinterpret_cast<CreateDispatcherQueueControllerFn>(
          GetProcAddress(core_messaging, "CreateDispatcherQueueController"));
      if (create_dq != nullptr) {
        DispatcherQueueOptions opts{};
        opts.dwSize = sizeof(DispatcherQueueOptions);
        opts.threadType = DQTYPE_THREAD_CURRENT;
        opts.apartmentType = DQTAT_COM_STA;
        create_dq(opts, &dispatcher_queue_controller_);
      }
    }

    HSTRING comp_class = nullptr;
    HSTRING_HEADER comp_header{};
    HRESULT hr = WindowsCreateStringReference(
        RuntimeClass_Windows_UI_Composition_Compositor,
        static_cast<UINT32>(
            wcslen(RuntimeClass_Windows_UI_Composition_Compositor)),
        &comp_header, &comp_class);
    if (FAILED(hr)) {
      return false;
    }
    ComPtr<IInspectable> inspectable_comp;
    hr = RoActivateInstance(comp_class, &inspectable_comp);
    if (FAILED(hr) || !inspectable_comp) {
      return false;
    }
    hr = inspectable_comp.As(&compositor_);
    if (FAILED(hr) || !compositor_) {
      return false;
    }

    const UINT flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    const D3D_FEATURE_LEVEL levels[] = {
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    };
    D3D_FEATURE_LEVEL chosen = D3D_FEATURE_LEVEL_11_0;
    hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, flags,
                           levels, static_cast<UINT>(std::size(levels)),
                           D3D11_SDK_VERSION, &d3d_device_, &chosen,
                           &d3d_context_);
    if (FAILED(hr)) {
      hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, flags,
                             levels, static_cast<UINT>(std::size(levels)),
                             D3D11_SDK_VERSION, &d3d_device_, &chosen,
                             &d3d_context_);
    }
    if (FAILED(hr) || !d3d_device_ || !d3d_context_) {
      return false;
    }

    ComPtr<IDXGIDevice> dxgi_device;
    hr = d3d_device_.As(&dxgi_device);
    if (FAILED(hr) || !dxgi_device) {
      return false;
    }

    HMODULE d3d11_mod = GetModuleHandleW(L"d3d11.dll");
    if (d3d11_mod == nullptr) {
      return false;
    }
    auto create_winrt_dev =
        reinterpret_cast<CreateDirect3D11DeviceFromDXGIDeviceFn>(GetProcAddress(
            d3d11_mod, "CreateDirect3D11DeviceFromDXGIDevice"));
    if (create_winrt_dev == nullptr) {
      return false;
    }
    ComPtr<IInspectable> inspectable_dev;
    hr = create_winrt_dev(dxgi_device.Get(), &inspectable_dev);
    if (FAILED(hr) || !inspectable_dev) {
      return false;
    }
    hr = inspectable_dev.As(&winrt_d3d_device_);
    if (FAILED(hr) || !winrt_d3d_device_) {
      return false;
    }

    D3D11_TEXTURE2D_DESC probe_desc{};
    probe_desc.Width = 4;
    probe_desc.Height = 4;
    probe_desc.MipLevels = 1;
    probe_desc.ArraySize = 1;
    probe_desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    probe_desc.SampleDesc.Count = 1;
    probe_desc.Usage = D3D11_USAGE_DEFAULT;
    probe_desc.BindFlags =
        D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
    probe_desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED;
    ComPtr<ID3D11Texture2D> probe_tex;
    if (SUCCEEDED(
            d3d_device_->CreateTexture2D(&probe_desc, nullptr, &probe_tex)) &&
        probe_tex) {
      ComPtr<IDXGIResource> dxgi_res;
      HANDLE shared_handle = nullptr;
      if (SUCCEEDED(probe_tex.As(&dxgi_res)) && dxgi_res &&
          SUCCEEDED(dxgi_res->GetSharedHandle(&shared_handle)) &&
          shared_handle != nullptr) {
        supports_dxgi_shared_ = true;
      }
    }

    valid_ = true;
    return true;
  }

  // Runs `fn` on the thread that initialized the runtime (the Flutter platform
  // thread) from a later Win32 message-loop turn via a message-only HWND, so
  // deferral dispatch works even when another plugin already owns the thread's
  // WinRT DispatcherQueue.
  bool PostToUiThread(std::function<void()> fn) {
    if (!fn || !task_queue_) {
      return false;
    }
    HWND hwnd = nullptr;
    {
      std::lock_guard<std::mutex> lock(task_queue_->mutex);
      if (task_queue_->closed || task_queue_->hwnd == nullptr ||
          !::IsWindow(task_queue_->hwnd)) {
        return false;
      }
      hwnd = task_queue_->hwnd;
      task_queue_->tasks.push_back(std::move(fn));
    }
    if (::PostMessageW(hwnd, kWmDrainUiTasks, 0, 0) != FALSE) {
      return true;
    }
    std::lock_guard<std::mutex> lock(task_queue_->mutex);
    if (!task_queue_->tasks.empty()) {
      task_queue_->tasks.pop_back();
    }
    return false;
  }

  bool supports_dxgi_shared() const { return supports_dxgi_shared_; }
  ABI::Windows::UI::Composition::ICompositor* compositor() const {
    return compositor_.Get();
  }
  ID3D11Device* d3d_device() const { return d3d_device_.Get(); }
  ID3D11DeviceContext* d3d_context() const { return d3d_context_.Get(); }
  ABI::Windows::Graphics::DirectX::Direct3D11::IDirect3DDevice*
  winrt_d3d_device() const {
    return winrt_d3d_device_.Get();
  }

 private:
  bool initialized_ = false;
  bool valid_ = false;
  bool supports_dxgi_shared_ = false;
  std::shared_ptr<UiTaskQueue> task_queue_ = std::make_shared<UiTaskQueue>();
  ComPtr<ABI::Windows::System::IDispatcherQueueController>
      dispatcher_queue_controller_;
  ComPtr<ABI::Windows::UI::Composition::ICompositor> compositor_;
  ComPtr<ID3D11Device> d3d_device_;
  ComPtr<ID3D11DeviceContext> d3d_context_;
  ComPtr<ABI::Windows::Graphics::DirectX::Direct3D11::IDirect3DDevice>
      winrt_d3d_device_;
};

// Builds a Win32 popup menu mirroring the WebView2 context menu model.
//
// Because the composition host HWND lives off-client at (-32000, -32000),
// letting WebView2 open its built-in popup would clamp the menu to monitor
// (0, 0). Instead, the host renders the menu items at the real screen cursor
// position and reports the chosen command back to WebView2.
void AppendContextMenuItems(HMENU menu,
                            ICoreWebView2ContextMenuItemCollection* items,
                            std::map<UINT, INT32>* command_ids,
                            UINT* next_id) {
  if (menu == nullptr || items == nullptr || command_ids == nullptr ||
      next_id == nullptr) {
    return;
  }
  UINT32 count = 0;
  if (FAILED(items->get_Count(&count))) {
    return;
  }
  bool last_was_separator = true;
  for (UINT32 i = 0; i < count; ++i) {
    ComPtr<ICoreWebView2ContextMenuItem> item;
    if (FAILED(items->GetValueAtIndex(i, &item)) || !item) {
      continue;
    }
    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND kind =
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND;
    item->get_Kind(&kind);
    if (kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR) {
      if (!last_was_separator) {
        AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
        last_was_separator = true;
      }
      continue;
    }

    std::wstring label;
    LPWSTR label_raw = nullptr;
    if (SUCCEEDED(item->get_Label(&label_raw)) && label_raw != nullptr) {
      label = label_raw;
      CoTaskMemFree(label_raw);
    }
    LPWSTR shortcut_raw = nullptr;
    if (SUCCEEDED(item->get_ShortcutKeyDescription(&shortcut_raw)) &&
        shortcut_raw != nullptr) {
      if (shortcut_raw[0] != L'\0') {
        label += L"\t";
        label += shortcut_raw;
      }
      CoTaskMemFree(shortcut_raw);
    }

    BOOL enabled = TRUE;
    item->get_IsEnabled(&enabled);
    UINT flags = MF_STRING | (enabled ? MF_ENABLED : MF_GRAYED);

    if (kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU) {
      ComPtr<ICoreWebView2ContextMenuItemCollection> children;
      if (SUCCEEDED(item->get_Children(&children)) && children) {
        HMENU submenu = CreatePopupMenu();
        if (submenu != nullptr) {
          AppendContextMenuItems(submenu, children.Get(), command_ids,
                                 next_id);
          AppendMenuW(menu, flags | MF_POPUP,
                      reinterpret_cast<UINT_PTR>(submenu), label.c_str());
          last_was_separator = false;
        }
      }
      continue;
    }

    if (kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_CHECK_BOX ||
        kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_RADIO) {
      BOOL checked = FALSE;
      item->get_IsChecked(&checked);
      if (checked) {
        flags |= MF_CHECKED;
      }
    }

    INT32 command_id = 0;
    item->get_CommandId(&command_id);
    const UINT menu_id = (*next_id)++;
    (*command_ids)[menu_id] = command_id;
    AppendMenuW(menu, flags, menu_id, label.c_str());
    last_was_separator = false;
  }
}

void TrimTrailingMenuSeparator(HMENU menu) {
  const int count = GetMenuItemCount(menu);
  if (count <= 0) {
    return;
  }
  MENUITEMINFOW info{};
  info.cbSize = sizeof(info);
  info.fMask = MIIM_FTYPE;
  if (GetMenuItemInfoW(menu, static_cast<UINT>(count - 1), TRUE, &info) &&
      (info.fType & MFT_SEPARATOR) != 0) {
    DeleteMenu(menu, static_cast<UINT>(count - 1), MF_BYPOSITION);
  }
}

constexpr const wchar_t kCompositionHostClassName[] =
    L"WebviewCoreCompositionHost";
constexpr const wchar_t kInternalWebViewHwndProp[] =
    L"WebviewCoreInternalHwnd";
constexpr const wchar_t kHostFlutterViewProp[] =
    L"WebviewCoreFlutterViewHwnd";
constexpr UINT_PTR kFlutterViewGuardSubclassId = 0x57564331u;

bool IsInternalWebViewWindow(HWND candidate) {
  HWND cur = candidate;
  for (int depth = 0; depth < 8 && cur != nullptr; ++depth) {
    if (::GetPropW(cur, kInternalWebViewHwndProp) != nullptr) {
      return true;
    }
    cur = ::GetParent(cur);
  }
  return false;
}

bool IsAnyMouseButtonPressed() {
  return (::GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0 ||
         (::GetAsyncKeyState(VK_RBUTTON) & 0x8000) != 0 ||
         (::GetAsyncKeyState(VK_MBUTTON) & 0x8000) != 0;
}

// Shields Flutter's view HWND (`FLUTTERVIEW`) from losing Win32 keyboard focus
// or mouse capture to WebView2's internal `Chrome_WidgetWin_*` child HWND.
//
// In composition hosting all pointer and keyboard input flows through Flutter
// (`Listener` / `Focus`) -> MethodChannel -> `SendMouseInput` / `ExecuteScript`.
// If WebView2 steals `GetCapture()` during a mouse press, `FLUTTERVIEW` misses
// `WM_LBUTTONUP` and WebView2 never receives `SendMouseInput(UP)`, wedging
// pointer input across the entire host window until the WebView is hidden.
LRESULT CALLBACK FlutterViewInputGuardSubclass(HWND hwnd,
                                               UINT msg,
                                               WPARAM wp,
                                               LPARAM lp,
                                               UINT_PTR subclass_id,
                                               DWORD_PTR /*ref_data*/) {
  if (msg == WM_NCDESTROY) {
    ::RemoveWindowSubclass(hwnd, FlutterViewInputGuardSubclass, subclass_id);
    return ::DefSubclassProc(hwnd, msg, wp, lp);
  }
  if (msg == WM_KILLFOCUS) {
    const HWND next = reinterpret_cast<HWND>(wp);
    if (IsInternalWebViewWindow(next)) {
      static thread_local bool restoring_focus = false;
      if (!restoring_focus) {
        restoring_focus = true;
        ::SetFocus(hwnd);
        restoring_focus = false;
      }
      return 0;
    }
  }
  if (msg == WM_CAPTURECHANGED) {
    const HWND next = reinterpret_cast<HWND>(lp);
    if (IsInternalWebViewWindow(next)) {
      static thread_local bool restoring_capture = false;
      if (!restoring_capture) {
        restoring_capture = true;
        if (IsAnyMouseButtonPressed()) {
          ::SetCapture(hwnd);
        } else {
          ::ReleaseCapture();
        }
        restoring_capture = false;
      }
      return 0;
    }
  }
  return ::DefSubclassProc(hwnd, msg, wp, lp);
}

void EnsureFlutterViewGuarded(HWND flutter_hwnd) {
  if (flutter_hwnd == nullptr || !::IsWindow(flutter_hwnd) ||
      flutter_hwnd == ::GetDesktopWindow()) {
    return;
  }
  DWORD_PTR existing = 0;
  if (!::GetWindowSubclass(flutter_hwnd, FlutterViewInputGuardSubclass,
                           kFlutterViewGuardSubclassId, &existing)) {
    ::SetWindowSubclass(flutter_hwnd, FlutterViewInputGuardSubclass,
                        kFlutterViewGuardSubclassId, 0);
  }
}

LRESULT CALLBACK CompositionHostWndProc(HWND hwnd,
                                        UINT msg,
                                        WPARAM wp,
                                        LPARAM lp) {
  switch (msg) {
    case WM_NCHITTEST:
      return HTTRANSPARENT;
    case WM_MOUSEACTIVATE:
      return MA_NOACTIVATE;
    case WM_PARENTNOTIFY:
      if (LOWORD(wp) == WM_CREATE) {
        const HWND child = reinterpret_cast<HWND>(lp);
        if (child != nullptr && ::IsWindow(child)) {
          ::SetPropW(child, kInternalWebViewHwndProp,
                     reinterpret_cast<HANDLE>(1));
        }
      }
      break;
    case WM_SETFOCUS: {
      const HWND flutter_view =
          static_cast<HWND>(::GetPropW(hwnd, kHostFlutterViewProp));
      if (flutter_view != nullptr && ::IsWindow(flutter_view) &&
          flutter_view != hwnd) {
        ::SetFocus(flutter_view);
        return 0;
      }
      break;
    }
    case WM_NCDESTROY:
      ::RemovePropW(hwnd, kInternalWebViewHwndProp);
      ::RemovePropW(hwnd, kHostFlutterViewProp);
      break;
    default:
      break;
  }
  return ::DefWindowProcW(hwnd, msg, wp, lp);
}

HWND CreateCompositionHostWindow(HWND top_hwnd, HWND flutter_hwnd) {
  static bool class_registered = false;
  const HINSTANCE instance = ::GetModuleHandleW(nullptr);
  if (!class_registered) {
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = CompositionHostWndProc;
    wc.hInstance = instance;
    wc.lpszClassName = kCompositionHostClassName;
    if (::RegisterClassExW(&wc) != 0 ||
        ::GetLastError() == ERROR_CLASS_ALREADY_EXISTS) {
      class_registered = true;
    }
  }
  if (!class_registered) {
    return nullptr;
  }
  const HWND parent = (top_hwnd != nullptr && ::IsWindow(top_hwnd) &&
                       top_hwnd != ::GetDesktopWindow())
                          ? top_hwnd
                          : flutter_hwnd;
  const DWORD style = (parent != nullptr && parent != ::GetDesktopWindow())
                          ? (WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS)
                          : WS_POPUP;
  const DWORD ex_style =
      WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_NOPARENTNOTIFY;
  const HWND host = ::CreateWindowExW(
      ex_style, kCompositionHostClassName, L"", style, -32000, -32000, 1, 1,
      parent, nullptr, instance, nullptr);
  if (host != nullptr) {
    ::SetPropW(host, kInternalWebViewHwndProp, reinterpret_cast<HANDLE>(1));
    if (flutter_hwnd != nullptr && flutter_hwnd != ::GetDesktopWindow()) {
      ::SetPropW(host, kHostFlutterViewProp, flutter_hwnd);
    }
  }
  return host;
}

class WebviewCorePlugin;

// Result sink for plugin-internal calls that have no Dart caller waiting.
class DiscardingMethodResult
    : public flutter::MethodResult<flutter::EncodableValue> {
 protected:
  void SuccessInternal(const flutter::EncodableValue* /*result*/) override {}
  void ErrorInternal(const std::string& /*error_code*/,
                     const std::string& /*error_message*/,
                     const flutter::EncodableValue* /*error_details*/)
      override {}
  void NotImplementedInternal() override {}
};

struct PendingOp {
  std::function<void()> execute;
  std::function<void()> cancel;
};

// Frame data read by the Flutter raster thread through the texture callbacks.
//
// It is reference counted and captured by those callbacks (instead of a raw
// `this`) so it can never dangle: the engine may still invoke a texture
// callback, and read the returned descriptor, after the owning view was
// disposed and destroyed (the engine unregisters textures asynchronously).
struct TextureFrameState {
  TextureFrameState() {
    gpu_surface.struct_size = sizeof(FlutterDesktopGpuSurfaceDescriptor);
    gpu_surface.handle = nullptr;
    gpu_surface.width = 0;
    gpu_surface.height = 0;
    gpu_surface.visible_width = 0;
    gpu_surface.visible_height = 0;
    gpu_surface.format = kFlutterDesktopPixelFormatBGRA8888;
    gpu_surface.release_callback = nullptr;
    gpu_surface.release_context = nullptr;

    pixel_buffer.buffer = nullptr;
    pixel_buffer.width = 0;
    pixel_buffer.height = 0;
    pixel_buffer.release_context = nullptr;
    pixel_buffer.release_callback = nullptr;
  }

  std::mutex mutex;
  HANDLE front_gpu_handle = nullptr;
  size_t gpu_width = 0;
  size_t gpu_height = 0;
  FlutterDesktopGpuSurfaceDescriptor gpu_surface{};

  std::vector<uint8_t> rgba_front;
  size_t front_width = 0;
  size_t front_height = 0;
  FlutterDesktopPixelBuffer pixel_buffer{};
};

// Shared D3D textures replaced by a resize. The engine imports them by legacy
// shared handle on its own thread/device, so they must outlive the last frame
// that referenced them.
struct RetiredSharedTextures {
  ComPtr<ID3D11Texture2D> textures[2];
  ULONGLONG retired_at_ms = 0;
};

constexpr ULONGLONG kRetiredTextureLifetimeMs = 3000;

class WindowsWebViewInstance
    : public std::enable_shared_from_this<WindowsWebViewInstance> {
 public:
  WindowsWebViewInstance(int64_t controller_id,
                         WebviewCorePlugin* plugin,
                         flutter::TextureRegistrar* texture_registrar,
                         CompositionGraphicsRuntime* gfx)
      : controller_id_(controller_id),
        plugin_(plugin),
        texture_registrar_(texture_registrar),
        gfx_(gfx) {
  }

  ~WindowsWebViewInstance() { Dispose(); }

  int64_t RegisterFlutterTexture() {
    use_gpu_surface_ = gfx_ != nullptr && gfx_->supports_dxgi_shared();
    if (use_gpu_surface_) {
      texture_variant_ = std::make_unique<flutter::TextureVariant>(
          flutter::GpuSurfaceTexture(
              kFlutterDesktopGpuSurfaceTypeDxgiSharedHandle,
              [state = frame_state_](size_t /*width*/, size_t /*height*/)
                  -> const FlutterDesktopGpuSurfaceDescriptor* {
                std::lock_guard<std::mutex> lock(state->mutex);
                if (state->front_gpu_handle == nullptr ||
                    state->gpu_width == 0 || state->gpu_height == 0) {
                  return nullptr;
                }
                state->gpu_surface.handle = state->front_gpu_handle;
                state->gpu_surface.width = state->gpu_width;
                state->gpu_surface.height = state->gpu_height;
                state->gpu_surface.visible_width = state->gpu_width;
                state->gpu_surface.visible_height = state->gpu_height;
                return &state->gpu_surface;
              }));
    } else {
      texture_variant_ = std::make_unique<flutter::TextureVariant>(
          flutter::PixelBufferTexture(
              [state = frame_state_](size_t /*width*/,
                                     size_t /*height*/)
                  -> const FlutterDesktopPixelBuffer* {
                std::lock_guard<std::mutex> lock(state->mutex);
                if (state->rgba_front.empty() || state->front_width == 0 ||
                    state->front_height == 0) {
                  return nullptr;
                }
                state->pixel_buffer.buffer = state->rgba_front.data();
                state->pixel_buffer.width = state->front_width;
                state->pixel_buffer.height = state->front_height;
                return &state->pixel_buffer;
              }));
    }
    texture_id_ = texture_registrar_->RegisterTexture(texture_variant_.get());
    return texture_id_;
  }

  void Initialize(HWND parent_hwnd,
                  ICoreWebView2Environment3* env3,
                  const flutter::EncodableMap& creation_args);

  void HandleCall(
      const std::string& method,
      const flutter::EncodableMap& args,
      std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result);

  void CancelPendingOps();
  void Dispose();

 private:
  void ApplyBackgroundColor(uint32_t argb);
  void SetSizeInternal(int width, int height, double scale_factor);
  void SetVisibilityInternal(bool visible);
  void HandlePointerEvent(const flutter::EncodableMap& args);
  void RestoreFlutterViewInputState(HWND prev_focus, HWND prev_capture);
  void HandleKeyEvent(const flutter::EncodableMap& args);
  HRESULT HandleContextMenuRequested(
      ICoreWebView2ContextMenuRequestedEventArgs* args);
  void ShowContextMenu(ICoreWebView2ContextMenuRequestedEventArgs* args,
                       ICoreWebView2ContextMenuItemCollection* items,
                       POINT screen_pt);
  void CancelActiveContextMenu();
  void RebuildChannelScript();
  void LoadHtmlInternal(const std::string& html, const std::string& base_url);
  void LoadRequestInternal(const std::string& url,
                           const std::string& method,
                           const flutter::EncodableValue* headers_val,
                           const flutter::EncodableValue* body_val);
  void ReplayCustomRequest(const std::string& url);
  void EnqueueOrRun(std::function<void()> op,
                    std::function<void()> on_cancel = nullptr);
  bool SetupCapturePipeline();
  void OnCaptureFrameArrived();
  void RetireSharedTextures();
  void PruneRetiredTextures();

  int64_t controller_id_ = 0;
  WebviewCorePlugin* plugin_ = nullptr;
  flutter::TextureRegistrar* texture_registrar_ = nullptr;
  CompositionGraphicsRuntime* gfx_ = nullptr;
  HWND parent_hwnd_ = nullptr;
  HWND top_hwnd_ = nullptr;
  HWND host_hwnd_ = nullptr;

  int64_t texture_id_ = -1;
  bool use_gpu_surface_ = false;
  std::unique_ptr<flutter::TextureVariant> texture_variant_;

  std::shared_ptr<TextureFrameState> frame_state_ =
      std::make_shared<TextureFrameState>();
  ComPtr<ID3D11Texture2D> shared_textures_[2];
  HANDLE shared_handles_[2] = {nullptr, nullptr};
  UINT shared_tex_width_ = 0;
  UINT shared_tex_height_ = 0;
  size_t shared_write_idx_ = 0;
  std::vector<RetiredSharedTextures> retired_textures_;

  ComPtr<ID3D11Texture2D> staging_texture_;
  UINT staging_width_ = 0;
  UINT staging_height_ = 0;

  ComPtr<ABI::Windows::UI::Composition::IContainerVisual> container_visual_;
  ComPtr<ABI::Windows::UI::Composition::IVisual> root_visual_;
  ComPtr<ABI::Windows::Graphics::Capture::IGraphicsCaptureItem> capture_item_;
  ComPtr<ABI::Windows::Graphics::Capture::IDirect3D11CaptureFramePool>
      frame_pool_;
  ComPtr<ABI::Windows::Graphics::Capture::IGraphicsCaptureSession>
      capture_session_;
  EventRegistrationToken frame_arrived_token_{};
  bool has_frame_token_ = false;

  ComPtr<ICoreWebView2Environment3> env3_;
  ComPtr<ICoreWebView2CompositionController> comp_controller_;
  ComPtr<ICoreWebView2Controller> controller_;
  ComPtr<ICoreWebView2> webview_;

  EventRegistrationToken nav_starting_token_{};
  EventRegistrationToken nav_completed_token_{};
  EventRegistrationToken source_changed_token_{};
  EventRegistrationToken web_message_token_{};
  EventRegistrationToken web_resource_token_{};
  bool has_web_resource_token_ = false;
  EventRegistrationToken context_menu_token_{};
  bool has_context_menu_token_ = false;
  EventRegistrationToken new_window_token_{};
  bool has_new_window_token_ = false;
  EventRegistrationToken move_focus_token_{};
  bool has_move_focus_token_ = false;
  std::wstring channel_script_id_;

  std::string large_html_utf8_;
  std::string large_html_virtual_uri_;
  std::wstring html_filter_pattern_;
  uint64_t large_html_seq_ = 0;

  std::string pending_custom_req_uri_;
  std::string pending_custom_req_method_;
  std::string pending_custom_req_headers_;
  std::vector<uint8_t> pending_custom_req_body_;
  bool has_pending_custom_req_ = false;

  int width_ = 800;
  int height_ = 600;
  double scale_factor_ = 1.0;
  bool visible_ = true;
  bool js_enabled_ = true;
  bool zoom_enabled_ = true;
  bool has_navigation_decision_ = false;
  std::string approved_navigation_uri_;
  uint32_t bg_color_argb_ = 0;
  std::wstring default_user_agent_;
  std::wstring custom_user_agent_;
  std::set<std::string> channel_names_;
  std::vector<PendingOp> pending_ops_;
  POINT last_context_screen_pt_{};
  bool has_last_context_screen_pt_ = false;
  bool showing_context_menu_ = false;
  bool ready_ = false;
  bool disposed_ = false;
};

class WebviewCorePlugin : public flutter::Plugin {
 public:
  static void RegisterWithRegistrar(
      flutter::PluginRegistrarWindows* registrar) {
    auto channel =
        std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
            registrar->messenger(), "com.infradise.webview_core",
            &flutter::StandardMethodCodec::GetInstance());

    auto plugin = std::make_unique<WebviewCorePlugin>(registrar,
                                                      std::move(channel));
    registrar->AddPlugin(std::move(plugin));
  }

  WebviewCorePlugin(
      flutter::PluginRegistrarWindows* registrar,
      std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel)
      : registrar_(registrar), channel_(std::move(channel)) {
    channel_->SetMethodCallHandler(
        [this](const auto& call, auto result) {
          HandleMethodCall(call, std::move(result));
        });
  }

  ~WebviewCorePlugin() override {
    disposed_ = true;
    for (auto& kv : instances_) {
      if (kv.second) {
        kv.second->Dispose();
      }
    }
    instances_.clear();
  }

  void SendEvent(const std::string& method, flutter::EncodableMap args) {
    if (disposed_ || !channel_) {
      return;
    }
    channel_->InvokeMethod(
        method, std::make_unique<flutter::EncodableValue>(std::move(args)));
  }

  void RequestNavigationDecision(int64_t controller_id,
                                 const std::string& url,
                                 bool is_main_frame,
                                 std::function<void(bool)> callback) {
    if (disposed_ || !channel_) {
      callback(true);
      return;
    }
    flutter::EncodableMap payload;
    payload[flutter::EncodableValue("controllerId")] =
        flutter::EncodableValue(controller_id);
    payload[flutter::EncodableValue("url")] = flutter::EncodableValue(url);
    payload[flutter::EncodableValue("isMainFrame")] =
        flutter::EncodableValue(is_main_frame);

    auto result_handler =
        std::make_unique<NavDecisionResult>(std::move(callback));
    channel_->InvokeMethod(
        "onNavigationRequest",
        std::make_unique<flutter::EncodableValue>(std::move(payload)),
        std::move(result_handler));
  }

 private:
  class NavDecisionResult
      : public flutter::MethodResult<flutter::EncodableValue> {
   public:
    explicit NavDecisionResult(std::function<void(bool)> cb)
        : cb_(std::move(cb)) {}

   protected:
    void SuccessInternal(const flutter::EncodableValue* reply) override {
      bool allow = true;
      if (reply != nullptr) {
        if (const auto* b = std::get_if<bool>(reply)) {
          allow = *b;
        }
      }
      cb_(allow);
    }
    void ErrorInternal(const std::string& /*code*/,
                       const std::string& /*msg*/,
                       const flutter::EncodableValue* /*details*/) override {
      cb_(true);
    }
    void NotImplementedInternal() override { cb_(true); }

   private:
    std::function<void(bool)> cb_;
  };

  void EnsureEnvironment(std::function<void(ICoreWebView2Environment3*)> cb) {
    if (env3_) {
      cb(env3_.Get());
      return;
    }
    env_waiters_.push_back(std::move(cb));
    if (creating_env_) {
      return;
    }
    creating_env_ = true;
    const std::wstring user_data = ResolveWritableUserDataFolder();
    const HRESULT hr = CreateCoreWebView2EnvironmentWithOptions(
        nullptr, user_data.c_str(), nullptr,
        Callback<ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler>(
            [this](HRESULT result, ICoreWebView2Environment* env) -> HRESULT {
              creating_env_ = false;
              if (SUCCEEDED(result) && env != nullptr) {
                env->QueryInterface(IID_PPV_ARGS(&env3_));
              }
              std::vector<std::function<void(ICoreWebView2Environment3*)>>
                  waiters;
              waiters.swap(env_waiters_);
              for (auto& fn : waiters) {
                fn(env3_.Get());
              }
              return S_OK;
            })
            .Get());
    if (FAILED(hr)) {
      creating_env_ = false;
      std::vector<std::function<void(ICoreWebView2Environment3*)>> waiters;
      waiters.swap(env_waiters_);
      for (auto& fn : waiters) {
        fn(nullptr);
      }
    }
  }

  void HandleMethodCall(
      const flutter::MethodCall<flutter::EncodableValue>& call,
      std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
    const auto* args = std::get_if<flutter::EncodableMap>(call.arguments());
    if (args == nullptr) {
      result->Error("INVALID_ARGS", "Expected argument map");
      return;
    }

    const std::string& method = call.method_name();
    if (method == "clearCookies" || method == "setCookie") {
      for (auto& kv : instances_) {
        if (kv.second) {
          kv.second->HandleCall(method, *args, std::move(result));
          return;
        }
      }
      if (method == "clearCookies") {
        pending_cookies_.clear();
        result->Success(flutter::EncodableValue(false));
      } else {
        pending_cookies_.push_back(*args);
        result->Success();
      }
      return;
    }

    int64_t controller_id = 0;
    if (!GetIntArg(*args, "controllerId", &controller_id)) {
      result->Error("INVALID_ARGS", "Missing controllerId");
      return;
    }

    if (method == "create") {
      auto old_it = instances_.find(controller_id);
      if (old_it != instances_.end()) {
        if (old_it->second) {
          old_it->second->Dispose();
        }
        instances_.erase(old_it);
      }

      if (!gfx_.EnsureInitialized()) {
        result->Success(flutter::EncodableValue(static_cast<int64_t>(-1)));
        return;
      }

      auto inst = std::make_shared<WindowsWebViewInstance>(
          controller_id, this, registrar_->texture_registrar(), &gfx_);
      const int64_t tex_id = inst->RegisterFlutterTexture();
      instances_[controller_id] = inst;
      if (!pending_cookies_.empty()) {
        std::vector<flutter::EncodableMap> cookies;
        cookies.swap(pending_cookies_);
        for (const auto& cookie : cookies) {
          inst->HandleCall("setCookie", cookie,
                           std::make_unique<DiscardingMethodResult>());
        }
      }
      HWND hwnd = nullptr;
      if (registrar_->GetView() != nullptr) {
        hwnd = registrar_->GetView()->GetNativeWindow();
      }
      if (hwnd == nullptr) {
        hwnd = GetDesktopWindow();
      }

      flutter::EncodableMap copied_args = *args;
      std::weak_ptr<WindowsWebViewInstance> weak_inst = inst;
      EnsureEnvironment([weak_inst, hwnd,
                         copied_args](ICoreWebView2Environment3* env3) {
        if (auto locked = weak_inst.lock()) {
          if (env3 != nullptr) {
            locked->Initialize(hwnd, env3, copied_args);
          } else {
            locked->CancelPendingOps();
          }
        }
      });

      result->Success(flutter::EncodableValue(tex_id));
      return;
    }

    if (method == "dispose") {
      auto it = instances_.find(controller_id);
      if (it != instances_.end()) {
        if (it->second) {
          it->second->Dispose();
        }
        instances_.erase(it);
      }
      result->Success();
      return;
    }

    auto it = instances_.find(controller_id);
    if (it == instances_.end() || !it->second) {
      result->Success();
      return;
    }
    it->second->HandleCall(method, *args, std::move(result));
  }

  flutter::PluginRegistrarWindows* registrar_ = nullptr;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  CompositionGraphicsRuntime gfx_;
  ComPtr<ICoreWebView2Environment3> env3_;
  bool creating_env_ = false;
  std::vector<std::function<void(ICoreWebView2Environment3*)>> env_waiters_;
  std::map<int64_t, std::shared_ptr<WindowsWebViewInstance>> instances_;
  // Cookies set through `WebViewCookieManager` before any view exists. WebView2
  // exposes the cookie jar only through a live view, so they are replayed into
  // the first view that is created.
  std::vector<flutter::EncodableMap> pending_cookies_;
  bool disposed_ = false;
};

bool WindowsWebViewInstance::SetupCapturePipeline() {
  if (gfx_ == nullptr || gfx_->compositor() == nullptr ||
      gfx_->winrt_d3d_device() == nullptr) {
    return false;
  }
  HRESULT hr =
      gfx_->compositor()->CreateContainerVisual(&container_visual_);
  if (FAILED(hr) || !container_visual_) {
    return false;
  }
  hr = container_visual_.As(&root_visual_);
  if (FAILED(hr) || !root_visual_) {
    return false;
  }

  ABI::Windows::Foundation::Numerics::Vector2 visual_size{};
  visual_size.X = static_cast<float>(width_ > 0 ? width_ : 1);
  visual_size.Y = static_cast<float>(height_ > 0 ? height_ : 1);
  root_visual_->put_Size(visual_size);
  root_visual_->put_IsVisible(visible_ ? TRUE : FALSE);

  HSTRING cap_class = nullptr;
  HSTRING_HEADER cap_header{};
  hr = WindowsCreateStringReference(
      RuntimeClass_Windows_Graphics_Capture_GraphicsCaptureItem,
      static_cast<UINT32>(
          wcslen(RuntimeClass_Windows_Graphics_Capture_GraphicsCaptureItem)),
      &cap_header, &cap_class);
  if (FAILED(hr)) {
    return false;
  }
  ComPtr<ABI::Windows::Graphics::Capture::IGraphicsCaptureItemStatics>
      item_statics;
  hr = RoGetActivationFactory(cap_class, IID_PPV_ARGS(&item_statics));
  if (FAILED(hr) || !item_statics) {
    return false;
  }
  hr = item_statics->CreateFromVisual(root_visual_.Get(), &capture_item_);
  if (FAILED(hr) || !capture_item_) {
    return false;
  }

  HSTRING pool_class = nullptr;
  HSTRING_HEADER pool_header{};
  hr = WindowsCreateStringReference(
      RuntimeClass_Windows_Graphics_Capture_Direct3D11CaptureFramePool,
      static_cast<UINT32>(wcslen(
          RuntimeClass_Windows_Graphics_Capture_Direct3D11CaptureFramePool)),
      &pool_header, &pool_class);
  if (FAILED(hr)) {
    return false;
  }
  ComPtr<ABI::Windows::Graphics::Capture::IDirect3D11CaptureFramePoolStatics>
      pool_statics;
  hr = RoGetActivationFactory(pool_class, IID_PPV_ARGS(&pool_statics));
  if (FAILED(hr) || !pool_statics) {
    return false;
  }

  ABI::Windows::Graphics::SizeInt32 pool_size{};
  pool_size.Width = width_ > 0 ? width_ : 1;
  pool_size.Height = height_ > 0 ? height_ : 1;
  hr = pool_statics->Create(
      gfx_->winrt_d3d_device(),
      ABI::Windows::Graphics::DirectX::DirectXPixelFormat_B8G8R8A8UIntNormalized,
      2, pool_size, &frame_pool_);
  if (FAILED(hr) || !frame_pool_) {
    return false;
  }

  std::weak_ptr<WindowsWebViewInstance> weak_self = shared_from_this();
  hr = frame_pool_->add_FrameArrived(
      Callback<ABI::Windows::Foundation::ITypedEventHandler<
          ABI::Windows::Graphics::Capture::Direct3D11CaptureFramePool*,
          IInspectable*>>(
          [weak_self](
              ABI::Windows::Graphics::Capture::IDirect3D11CaptureFramePool*,
              IInspectable*) -> HRESULT {
            if (auto self = weak_self.lock()) {
              self->OnCaptureFrameArrived();
            }
            return S_OK;
          })
          .Get(),
      &frame_arrived_token_);
  if (SUCCEEDED(hr)) {
    has_frame_token_ = true;
  }

  hr = frame_pool_->CreateCaptureSession(capture_item_.Get(),
                                         &capture_session_);
  if (FAILED(hr) || !capture_session_) {
    return false;
  }

  ComPtr<ABI::Windows::Graphics::Capture::IGraphicsCaptureSession2> session2;
  if (SUCCEEDED(capture_session_.As(&session2)) && session2) {
    session2->put_IsCursorCaptureEnabled(FALSE);
  }
  ComPtr<ABI::Windows::Graphics::Capture::IGraphicsCaptureSession3> session3;
  if (SUCCEEDED(capture_session_.As(&session3)) && session3) {
    session3->put_IsBorderRequired(FALSE);
  }

  hr = capture_session_->StartCapture();
  return SUCCEEDED(hr);
}

void WindowsWebViewInstance::RetireSharedTextures() {
  if (!shared_textures_[0] && !shared_textures_[1]) {
    return;
  }
  RetiredSharedTextures retired;
  retired.textures[0] = std::move(shared_textures_[0]);
  retired.textures[1] = std::move(shared_textures_[1]);
  retired.retired_at_ms = GetTickCount64();
  retired_textures_.push_back(std::move(retired));
  shared_textures_[0].Reset();
  shared_textures_[1].Reset();
}

void WindowsWebViewInstance::PruneRetiredTextures() {
  if (retired_textures_.empty()) {
    return;
  }
  const ULONGLONG now = GetTickCount64();
  retired_textures_.erase(
      std::remove_if(retired_textures_.begin(), retired_textures_.end(),
                     [now](const RetiredSharedTextures& r) {
                       return now - r.retired_at_ms >
                              kRetiredTextureLifetimeMs;
                     }),
      retired_textures_.end());
}

void WindowsWebViewInstance::OnCaptureFrameArrived() {
  if (disposed_ || !frame_pool_ || gfx_ == nullptr) {
    return;
  }
  ComPtr<ABI::Windows::Graphics::Capture::IDirect3D11CaptureFrame> frame;
  HRESULT hr = frame_pool_->TryGetNextFrame(&frame);
  if (FAILED(hr) || !frame) {
    return;
  }

  ComPtr<ABI::Windows::Graphics::DirectX::Direct3D11::IDirect3DSurface> surface;
  hr = frame->get_Surface(&surface);
  if (SUCCEEDED(hr) && surface) {
    ComPtr<IDirect3DDxgiInterfaceAccessLocal> dxgi_access;
    if (SUCCEEDED(surface.As(&dxgi_access)) && dxgi_access) {
      ComPtr<ID3D11Texture2D> src_tex;
      if (SUCCEEDED(dxgi_access->GetInterface(IID_PPV_ARGS(&src_tex))) &&
          src_tex) {
        D3D11_TEXTURE2D_DESC desc{};
        src_tex->GetDesc(&desc);
        if (desc.Width > 0 && desc.Height > 0 &&
            desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM) {
          if (use_gpu_surface_) {
            if (!shared_textures_[0] || !shared_textures_[1] ||
                shared_tex_width_ != desc.Width ||
                shared_tex_height_ != desc.Height) {
              {
                std::lock_guard<std::mutex> lock(frame_state_->mutex);
                frame_state_->front_gpu_handle = nullptr;
              }
              RetireSharedTextures();
              shared_handles_[0] = nullptr;
              shared_handles_[1] = nullptr;

              D3D11_TEXTURE2D_DESC sh_desc = desc;
              sh_desc.MipLevels = 1;
              sh_desc.ArraySize = 1;
              sh_desc.SampleDesc.Count = 1;
              sh_desc.Usage = D3D11_USAGE_DEFAULT;
              sh_desc.BindFlags =
                  D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
              sh_desc.CPUAccessFlags = 0;
              sh_desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED;

              bool ok = true;
              for (int i = 0; i < 2; ++i) {
                if (FAILED(gfx_->d3d_device()->CreateTexture2D(
                        &sh_desc, nullptr, &shared_textures_[i])) ||
                    !shared_textures_[i]) {
                  ok = false;
                  break;
                }
                ComPtr<IDXGIResource> dxgi_res;
                if (FAILED(shared_textures_[i].As(&dxgi_res)) || !dxgi_res ||
                    FAILED(dxgi_res->GetSharedHandle(&shared_handles_[i])) ||
                    shared_handles_[i] == nullptr) {
                  ok = false;
                  break;
                }
              }
              if (ok) {
                shared_tex_width_ = desc.Width;
                shared_tex_height_ = desc.Height;
              } else {
                RetireSharedTextures();
                shared_handles_[0] = nullptr;
                shared_handles_[1] = nullptr;
              }
            }

            const size_t idx = shared_write_idx_ & 1u;
            if (shared_textures_[idx] && shared_handles_[idx] != nullptr) {
              gfx_->d3d_context()->CopyResource(shared_textures_[idx].Get(),
                                                src_tex.Get());
              gfx_->d3d_context()->Flush();
              {
                std::lock_guard<std::mutex> lock(frame_state_->mutex);
                frame_state_->front_gpu_handle = shared_handles_[idx];
                frame_state_->gpu_width = static_cast<size_t>(desc.Width);
                frame_state_->gpu_height = static_cast<size_t>(desc.Height);
                shared_write_idx_ = idx ^ 1u;
              }
              PruneRetiredTextures();
              if (texture_registrar_ != nullptr && texture_id_ >= 0) {
                texture_registrar_->MarkTextureFrameAvailable(texture_id_);
              }
            }
          } else {
            if (!staging_texture_ || staging_width_ != desc.Width ||
                staging_height_ != desc.Height) {
              staging_texture_.Reset();
              D3D11_TEXTURE2D_DESC st_desc = desc;
              st_desc.Usage = D3D11_USAGE_STAGING;
              st_desc.BindFlags = 0;
              st_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
              st_desc.MiscFlags = 0;
              if (SUCCEEDED(gfx_->d3d_device()->CreateTexture2D(
                      &st_desc, nullptr, &staging_texture_))) {
                staging_width_ = desc.Width;
                staging_height_ = desc.Height;
              }
            }
            if (staging_texture_) {
              gfx_->d3d_context()->CopyResource(staging_texture_.Get(),
                                                src_tex.Get());
              D3D11_MAPPED_SUBRESOURCE mapped{};
              if (SUCCEEDED(gfx_->d3d_context()->Map(
                      staging_texture_.Get(), 0, D3D11_MAP_READ, 0, &mapped))) {
                {
                  std::lock_guard<std::mutex> lock(frame_state_->mutex);
                  const size_t w = static_cast<size_t>(desc.Width);
                  const size_t h = static_cast<size_t>(desc.Height);
                  frame_state_->rgba_front.resize(w * h * 4);
                  for (size_t row = 0; row < h; ++row) {
                    const auto* src_row = reinterpret_cast<const uint32_t*>(
                        static_cast<const uint8_t*>(mapped.pData) +
                        row * mapped.RowPitch);
                    auto* dst_row = reinterpret_cast<uint32_t*>(
                        frame_state_->rgba_front.data() + row * w * 4);
                    for (size_t col = 0; col < w; ++col) {
                      const uint32_t px = src_row[col];
                      dst_row[col] = (px & 0xFF00FF00u) |
                                     ((px & 0x00FF0000u) >> 16) |
                                     ((px & 0x000000FFu) << 16);
                    }
                  }
                  frame_state_->front_width = w;
                  frame_state_->front_height = h;
                }
                gfx_->d3d_context()->Unmap(staging_texture_.Get(), 0);
                if (texture_registrar_ != nullptr && texture_id_ >= 0) {
                  texture_registrar_->MarkTextureFrameAvailable(texture_id_);
                }
              }
            }
          }
        }
      }
    }
  }

  ComPtr<ABI::Windows::Foundation::IClosable> closable_frame;
  if (SUCCEEDED(frame.As(&closable_frame)) && closable_frame) {
    closable_frame->Close();
  }
}

void WindowsWebViewInstance::CancelPendingOps() {
  std::vector<PendingOp> ops;
  ops.swap(pending_ops_);
  for (auto& op : ops) {
    if (op.cancel) {
      op.cancel();
    }
  }
}

void WindowsWebViewInstance::Initialize(
    HWND parent_hwnd,
    ICoreWebView2Environment3* env3,
    const flutter::EncodableMap& creation_args) {
  if (disposed_ || env3 == nullptr) {
    CancelPendingOps();
    return;
  }
  parent_hwnd_ = parent_hwnd;
  if (parent_hwnd != nullptr && parent_hwnd != ::GetDesktopWindow() &&
      ::IsWindow(parent_hwnd)) {
    top_hwnd_ = ::GetAncestor(parent_hwnd, GA_ROOT);
    EnsureFlutterViewGuarded(parent_hwnd);
  }
  host_hwnd_ = CreateCompositionHostWindow(top_hwnd_, parent_hwnd_);
  env3_ = env3;
  js_enabled_ = GetBoolArg(creation_args, "javaScriptEnabled", true);
  zoom_enabled_ = GetBoolArg(creation_args, "zoomEnabled", true);
  has_navigation_decision_ =
      GetBoolArg(creation_args, "hasNavigationDecision", false);

  int64_t raw_bg = 0;
  if (GetIntArg(creation_args, "backgroundColor", &raw_bg)) {
    bg_color_argb_ = static_cast<uint32_t>(raw_bg);
  }

  bool has_ua = false;
  const std::string ua = GetStringArg(creation_args, "userAgent", &has_ua);
  if (has_ua && !ua.empty()) {
    custom_user_agent_ = Utf8ToWide(ua);
  }

  if (const auto* ch_val = LookupMap(creation_args, "channelNames")) {
    if (const auto* list = std::get_if<flutter::EncodableList>(ch_val)) {
      for (const auto& item : *list) {
        if (const auto* name = std::get_if<std::string>(&item)) {
          if (!name->empty()) {
            channel_names_.insert(*name);
          }
        }
      }
    }
  }

  if (!SetupCapturePipeline()) {
    CancelPendingOps();
    return;
  }

  const HWND composition_parent =
      host_hwnd_ != nullptr ? host_hwnd_ : parent_hwnd_;
  std::weak_ptr<WindowsWebViewInstance> weak_self = shared_from_this();
  env3_->CreateCoreWebView2CompositionController(
      composition_parent,
      Callback<ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler>(
          [weak_self, creation_args](
              HRESULT result,
              ICoreWebView2CompositionController* comp) -> HRESULT {
            auto self = weak_self.lock();
            if (!self) {
              return S_OK;
            }
            if (self->disposed_ || FAILED(result) || comp == nullptr) {
              self->CancelPendingOps();
              return S_OK;
            }
            self->comp_controller_ = comp;
            if (FAILED(comp->QueryInterface(IID_PPV_ARGS(&self->controller_))) ||
                !self->controller_) {
              self->CancelPendingOps();
              return S_OK;
            }
            if (FAILED(self->controller_->get_CoreWebView2(&self->webview_)) ||
                !self->webview_) {
              self->CancelPendingOps();
              return S_OK;
            }

            const HWND scan_parent = self->host_hwnd_ != nullptr
                                         ? self->host_hwnd_
                                         : self->parent_hwnd_;
            if (scan_parent != nullptr && ::IsWindow(scan_parent)) {
              for (HWND child = ::GetWindow(scan_parent, GW_CHILD);
                   child != nullptr;
                   child = ::GetWindow(child, GW_HWNDNEXT)) {
                if (child != self->parent_hwnd_) {
                  ::SetPropW(child, kInternalWebViewHwndProp,
                             reinterpret_cast<HANDLE>(1));
                }
              }
            }

            if (SUCCEEDED(self->controller_->add_MoveFocusRequested(
                    Callback<ICoreWebView2MoveFocusRequestedEventHandler>(
                        [weak_self](
                            ICoreWebView2Controller* /*sender*/,
                            ICoreWebView2MoveFocusRequestedEventArgs* args)
                            -> HRESULT {
                          if (args != nullptr) {
                            args->put_Handled(TRUE);
                          }
                          if (auto inst = weak_self.lock()) {
                            inst->RestoreFlutterViewInputState(
                                inst->parent_hwnd_, ::GetCapture());
                          }
                          return S_OK;
                        })
                        .Get(),
                    &self->move_focus_token_))) {
              self->has_move_focus_token_ = true;
            }

            if (self->container_visual_) {
              self->comp_controller_->put_RootVisualTarget(
                  self->container_visual_.Get());
            }

            ComPtr<ICoreWebView2Settings> settings;
            if (SUCCEEDED(self->webview_->get_Settings(&settings)) && settings) {
              settings->put_IsScriptEnabled(self->js_enabled_ ? TRUE : FALSE);
              settings->put_IsWebMessageEnabled(TRUE);
              settings->put_IsStatusBarEnabled(FALSE);
              settings->put_IsZoomControlEnabled(
                  self->zoom_enabled_ ? TRUE : FALSE);
              ComPtr<ICoreWebView2Settings2> settings2;
              if (SUCCEEDED(settings.As(&settings2)) && settings2) {
                LPWSTR ua_ptr = nullptr;
                if (SUCCEEDED(settings2->get_UserAgent(&ua_ptr)) &&
                    ua_ptr != nullptr) {
                  self->default_user_agent_ = ua_ptr;
                  CoTaskMemFree(ua_ptr);
                }
                if (!self->custom_user_agent_.empty()) {
                  settings2->put_UserAgent(self->custom_user_agent_.c_str());
                }
              }
            }

            self->ApplyBackgroundColor(self->bg_color_argb_);
            self->SetSizeInternal(self->width_, self->height_,
                                  self->scale_factor_);
            self->controller_->put_IsVisible(self->visible_ ? TRUE : FALSE);
            self->RebuildChannelScript();

            self->webview_->AddWebResourceRequestedFilter(
                L"http://webview-core.invalid/*",
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT);
            if (SUCCEEDED(self->webview_->add_WebResourceRequested(
                    Callback<ICoreWebView2WebResourceRequestedEventHandler>(
                        [weak_self](
                            ICoreWebView2* /*sender*/,
                            ICoreWebView2WebResourceRequestedEventArgs* args)
                            -> HRESULT {
                          auto inst = weak_self.lock();
                          if (!inst || inst->disposed_ || args == nullptr ||
                              !inst->env3_) {
                            return S_OK;
                          }
                          ComPtr<ICoreWebView2WebResourceRequest> req;
                          if (FAILED(args->get_Request(&req)) || !req) {
                            return S_OK;
                          }
                          LPWSTR uri_w = nullptr;
                          req->get_Uri(&uri_w);
                          const std::string uri = WideToUtf8(uri_w);
                          if (uri_w != nullptr) {
                            CoTaskMemFree(uri_w);
                          }
                          if (!inst->large_html_virtual_uri_.empty() &&
                              uri == inst->large_html_virtual_uri_) {
                            ComPtr<IStream> mem_stream;
                            mem_stream.Attach(SHCreateMemStream(
                                reinterpret_cast<const BYTE*>(
                                    inst->large_html_utf8_.data()),
                                static_cast<UINT>(
                                    inst->large_html_utf8_.size())));
                            ComPtr<ICoreWebView2WebResourceResponse> resp;
                            if (mem_stream &&
                                SUCCEEDED(
                                    inst->env3_->CreateWebResourceResponse(
                                        mem_stream.Get(), 200, L"OK",
                                        L"Content-Type: text/html; "
                                        L"charset=utf-8\r\n"
                                         L"Cache-Control: no-store",
                                        &resp)) &&
                                resp) {
                              args->put_Response(resp.Get());
                            }
                          }
                          return S_OK;
                        })
                        .Get(),
                    &self->web_resource_token_))) {
              self->has_web_resource_token_ = true;
            }

            self->webview_->add_NavigationStarting(
                Callback<ICoreWebView2NavigationStartingEventHandler>(
                    [weak_self](
                        ICoreWebView2* /*sender*/,
                        ICoreWebView2NavigationStartingEventArgs* args)
                        -> HRESULT {
                      auto inst = weak_self.lock();
                      if (!inst || inst->disposed_ || args == nullptr) {
                        return S_OK;
                      }
                      LPWSTR uri_w = nullptr;
                      args->get_Uri(&uri_w);
                      const std::string uri = WideToUtf8(uri_w);
                      if (uri_w != nullptr) {
                        CoTaskMemFree(uri_w);
                      }
                      const bool is_virtual_inline =
                          !inst->large_html_virtual_uri_.empty() &&
                          uri == inst->large_html_virtual_uri_;
                      if (!is_virtual_inline &&
                          inst->has_navigation_decision_ &&
                          inst->plugin_ != nullptr && uri != "about:blank" &&
                          uri.rfind("data:", 0) != 0) {
                        if (inst->approved_navigation_uri_ == uri) {
                          inst->approved_navigation_uri_.clear();
                        } else {
                          args->put_Cancel(TRUE);
                          inst->plugin_->RequestNavigationDecision(
                              inst->controller_id_, uri, true,
                              [weak_self, uri](bool allow) {
                                auto self_inner = weak_self.lock();
                                if (!self_inner || self_inner->disposed_ ||
                                    !allow || !self_inner->webview_) {
                                  return;
                                }
                                self_inner->approved_navigation_uri_ = uri;
                                if (self_inner->has_pending_custom_req_ &&
                                    self_inner->pending_custom_req_uri_ ==
                                        uri) {
                                  self_inner->ReplayCustomRequest(uri);
                                } else {
                                  const std::wstring wuri = Utf8ToWide(uri);
                                  self_inner->webview_->Navigate(wuri.c_str());
                                }
                              });
                          return S_OK;
                        }
                      }
                      if (inst->plugin_ != nullptr) {
                        flutter::EncodableMap ev;
                        ev[flutter::EncodableValue("controllerId")] =
                            flutter::EncodableValue(inst->controller_id_);
                        ev[flutter::EncodableValue("url")] =
                            flutter::EncodableValue(
                                is_virtual_inline ? std::string("about:blank")
                                                  : uri);
                        inst->plugin_->SendEvent("onPageStarted",
                                                 std::move(ev));
                      }
                      return S_OK;
                    })
                    .Get(),
                &self->nav_starting_token_);

            // Composition hosting renders no default context menu: surface the
            // Edge menu (Back, Forward, Reload, Copy, ...) as a native popup.
            ComPtr<ICoreWebView2_11> webview11;
            if (SUCCEEDED(self->webview_.As(&webview11)) && webview11) {
              if (SUCCEEDED(webview11->add_ContextMenuRequested(
                      Callback<ICoreWebView2ContextMenuRequestedEventHandler>(
                          [weak_self](
                              ICoreWebView2* /*sender*/,
                              ICoreWebView2ContextMenuRequestedEventArgs* args)
                              -> HRESULT {
                            auto inst = weak_self.lock();
                            if (!inst || inst->disposed_ || args == nullptr) {
                              return S_OK;
                            }
                            return inst->HandleContextMenuRequested(args);
                          })
                          .Get(),
                      &self->context_menu_token_))) {
                self->has_context_menu_token_ = true;
              }
            }

            // target="_blank" / window.open(): embedded views have no tab
            // strip, so open http(s) targets in place.
            if (SUCCEEDED(self->webview_->add_NewWindowRequested(
                    Callback<ICoreWebView2NewWindowRequestedEventHandler>(
                        [weak_self](
                            ICoreWebView2* sender,
                            ICoreWebView2NewWindowRequestedEventArgs* args)
                            -> HRESULT {
                          auto inst = weak_self.lock();
                          if (!inst || inst->disposed_ || args == nullptr) {
                            return S_OK;
                          }
                          args->put_Handled(TRUE);
                          LPWSTR uri_w = nullptr;
                          if (SUCCEEDED(args->get_Uri(&uri_w)) &&
                              uri_w != nullptr) {
                            const std::wstring target(uri_w);
                            CoTaskMemFree(uri_w);
                            if (sender != nullptr &&
                                (target.rfind(L"http://", 0) == 0 ||
                                 target.rfind(L"https://", 0) == 0)) {
                              sender->Navigate(target.c_str());
                            }
                          }
                          return S_OK;
                        })
                        .Get(),
                    &self->new_window_token_))) {
              self->has_new_window_token_ = true;
            }

            self->webview_->add_NavigationCompleted(
                Callback<ICoreWebView2NavigationCompletedEventHandler>(
                    [weak_self](
                        ICoreWebView2* sender,
                        ICoreWebView2NavigationCompletedEventArgs* args)
                        -> HRESULT {
                      auto inst = weak_self.lock();
                      if (!inst || inst->disposed_ || inst->plugin_ == nullptr ||
                          sender == nullptr || args == nullptr) {
                        return S_OK;
                      }
                      LPWSTR uri_w = nullptr;
                      sender->get_Source(&uri_w);
                      std::string uri = WideToUtf8(uri_w);
                      if (uri_w != nullptr) {
                        CoTaskMemFree(uri_w);
                      }
                      if (!inst->large_html_virtual_uri_.empty() &&
                          uri == inst->large_html_virtual_uri_) {
                        uri = "about:blank";
                      }
                      BOOL is_success = TRUE;
                      args->get_IsSuccess(&is_success);
                      if (is_success) {
                        flutter::EncodableMap prog;
                        prog[flutter::EncodableValue("controllerId")] =
                            flutter::EncodableValue(inst->controller_id_);
                        prog[flutter::EncodableValue("progress")] =
                            flutter::EncodableValue(100);
                        inst->plugin_->SendEvent("onProgress", std::move(prog));

                        flutter::EncodableMap fin;
                        fin[flutter::EncodableValue("controllerId")] =
                            flutter::EncodableValue(inst->controller_id_);
                        fin[flutter::EncodableValue("url")] =
                            flutter::EncodableValue(uri);
                        inst->plugin_->SendEvent("onPageFinished",
                                                 std::move(fin));
                      } else {
                        COREWEBVIEW2_WEB_ERROR_STATUS status =
                            COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                        args->get_WebErrorStatus(&status);
                        if (status !=
                            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED) {
                          flutter::EncodableMap err;
                          err[flutter::EncodableValue("controllerId")] =
                              flutter::EncodableValue(inst->controller_id_);
                          err[flutter::EncodableValue("errorCode")] =
                              flutter::EncodableValue(
                                  static_cast<int32_t>(status));
                          err[flutter::EncodableValue("description")] =
                              flutter::EncodableValue(
                                  std::string("Navigation failed"));
                          err[flutter::EncodableValue("errorType")] =
                              flutter::EncodableValue(std::string("unknown"));
                          err[flutter::EncodableValue("isForMainFrame")] =
                              flutter::EncodableValue(true);
                          err[flutter::EncodableValue("url")] =
                              flutter::EncodableValue(uri);
                          inst->plugin_->SendEvent("onWebResourceError",
                                                   std::move(err));
                        }
                      }
                      return S_OK;
                    })
                    .Get(),
                &self->nav_completed_token_);

            self->webview_->add_SourceChanged(
                Callback<ICoreWebView2SourceChangedEventHandler>(
                    [weak_self](
                        ICoreWebView2* sender,
                        ICoreWebView2SourceChangedEventArgs* /*args*/)
                        -> HRESULT {
                      auto inst = weak_self.lock();
                      if (!inst || inst->disposed_ || inst->plugin_ == nullptr ||
                          sender == nullptr) {
                        return S_OK;
                      }
                      LPWSTR uri_w = nullptr;
                      sender->get_Source(&uri_w);
                      std::string uri = WideToUtf8(uri_w);
                      if (uri_w != nullptr) {
                        CoTaskMemFree(uri_w);
                      }
                      if (!inst->large_html_virtual_uri_.empty() &&
                          uri == inst->large_html_virtual_uri_) {
                        uri = "about:blank";
                      }
                      flutter::EncodableMap ev;
                      ev[flutter::EncodableValue("controllerId")] =
                          flutter::EncodableValue(inst->controller_id_);
                      ev[flutter::EncodableValue("url")] =
                          flutter::EncodableValue(uri);
                      inst->plugin_->SendEvent("onUrlChange", std::move(ev));
                      return S_OK;
                    })
                    .Get(),
                &self->source_changed_token_);

            self->webview_->add_WebMessageReceived(
                Callback<ICoreWebView2WebMessageReceivedEventHandler>(
                    [weak_self](
                        ICoreWebView2* /*sender*/,
                        ICoreWebView2WebMessageReceivedEventArgs* args)
                        -> HRESULT {
                      auto inst = weak_self.lock();
                      if (!inst || inst->disposed_ || inst->plugin_ == nullptr ||
                          args == nullptr) {
                        return S_OK;
                      }
                      LPWSTR msg_w = nullptr;
                      if (SUCCEEDED(args->TryGetWebMessageAsString(&msg_w)) &&
                          msg_w != nullptr) {
                        const std::string raw = WideToUtf8(msg_w);
                        CoTaskMemFree(msg_w);
                        const std::string prefix = "\x01__wvcore__:";
                        if (raw.rfind(prefix, 0) == 0) {
                          const size_t nl = raw.find('\n', prefix.size());
                          if (nl != std::string::npos) {
                            const std::string ch =
                                raw.substr(prefix.size(), nl - prefix.size());
                            const std::string payload = raw.substr(nl + 1);
                            flutter::EncodableMap ev;
                            ev[flutter::EncodableValue("controllerId")] =
                                flutter::EncodableValue(inst->controller_id_);
                            ev[flutter::EncodableValue("channel")] =
                                flutter::EncodableValue(ch);
                            ev[flutter::EncodableValue("message")] =
                                flutter::EncodableValue(payload);
                            inst->plugin_->SendEvent("onJavaScriptMessage",
                                                     std::move(ev));
                          }
                        }
                      }
                      return S_OK;
                    })
                    .Get(),
                &self->web_message_token_);

            bool has_html = false;
            const std::string init_html =
                GetStringArg(creation_args, "initialHtml", &has_html);
            bool has_url = false;
            const std::string init_url =
                GetStringArg(creation_args, "initialUrl", &has_url);
            if (has_html) {
              const std::string init_base =
                  GetStringArg(creation_args, "initialBaseUrl");
              self->LoadHtmlInternal(init_html, init_base);
            } else if (has_url && !init_url.empty()) {
              const std::string m =
                  GetStringArg(creation_args, "initialMethod");
              self->LoadRequestInternal(
                  init_url, m.empty() ? "GET" : m,
                  LookupMap(creation_args, "initialHeaders"),
                  LookupMap(creation_args, "initialBody"));
            }

            self->ready_ = true;
            std::vector<PendingOp> ops;
            ops.swap(self->pending_ops_);
            for (auto& op : ops) {
              if (!self->disposed_) {
                if (op.execute) {
                  op.execute();
                }
              } else if (op.cancel) {
                op.cancel();
              }
            }

            if (self->plugin_ != nullptr) {
              flutter::EncodableMap ready_ev;
              ready_ev[flutter::EncodableValue("controllerId")] =
                  flutter::EncodableValue(self->controller_id_);
              self->plugin_->SendEvent("onViewCreated", std::move(ready_ev));
            }
            return S_OK;
          })
          .Get());
}

void WindowsWebViewInstance::Dispose() {
  if (disposed_) {
    return;
  }
  disposed_ = true;
  CancelActiveContextMenu();
  CancelPendingOps();

  if (webview_) {
    if (has_web_resource_token_) {
      webview_->remove_WebResourceRequested(web_resource_token_);
      has_web_resource_token_ = false;
    }
    if (has_context_menu_token_) {
      ComPtr<ICoreWebView2_11> webview11;
      if (SUCCEEDED(webview_.As(&webview11)) && webview11) {
        webview11->remove_ContextMenuRequested(context_menu_token_);
      }
      has_context_menu_token_ = false;
    }
    if (has_new_window_token_) {
      webview_->remove_NewWindowRequested(new_window_token_);
      has_new_window_token_ = false;
    }
    webview_->remove_NavigationStarting(nav_starting_token_);
    webview_->remove_NavigationCompleted(nav_completed_token_);
    webview_->remove_SourceChanged(source_changed_token_);
    webview_->remove_WebMessageReceived(web_message_token_);
    webview_->Stop();
    webview_.Reset();
  }
  if (comp_controller_) {
    comp_controller_->put_RootVisualTarget(nullptr);
    comp_controller_.Reset();
  }
  if (controller_) {
    if (has_move_focus_token_) {
      controller_->remove_MoveFocusRequested(move_focus_token_);
      has_move_focus_token_ = false;
    }
    controller_->Close();
    controller_.Reset();
  }
  if (host_hwnd_ != nullptr) {
    if (::IsWindow(host_hwnd_)) {
      for (HWND child = ::GetWindow(host_hwnd_, GW_CHILD); child != nullptr;
           child = ::GetWindow(child, GW_HWNDNEXT)) {
        ::RemovePropW(child, kInternalWebViewHwndProp);
      }
      ::DestroyWindow(host_hwnd_);
    }
    host_hwnd_ = nullptr;
  }
  if (frame_pool_ && has_frame_token_) {
    frame_pool_->remove_FrameArrived(frame_arrived_token_);
    has_frame_token_ = false;
  }
  if (capture_session_) {
    ComPtr<ABI::Windows::Foundation::IClosable> closable;
    if (SUCCEEDED(capture_session_.As(&closable)) && closable) {
      closable->Close();
    }
    capture_session_.Reset();
  }
  if (frame_pool_) {
    ComPtr<ABI::Windows::Foundation::IClosable> closable;
    if (SUCCEEDED(frame_pool_.As(&closable)) && closable) {
      closable->Close();
    }
    frame_pool_.Reset();
  }
  capture_item_.Reset();
  root_visual_.Reset();
  container_visual_.Reset();
  staging_texture_.Reset();

  if (texture_registrar_ != nullptr && texture_id_ >= 0) {
    const int64_t id_to_remove = texture_id_;
    texture_id_ = -1;
    std::shared_ptr<flutter::TextureVariant> keep_alive(
        std::move(texture_variant_));
    // Hand every texture the engine may still import to the unregister
    // callback: it runs only once the engine no longer uses this texture.
    auto keep_textures = std::make_shared<std::vector<RetiredSharedTextures>>(
        std::move(retired_textures_));
    RetiredSharedTextures current;
    current.textures[0] = shared_textures_[0];
    current.textures[1] = shared_textures_[1];
    keep_textures->push_back(std::move(current));
    auto keep_state = frame_state_;
    {
      std::lock_guard<std::mutex> lock(frame_state_->mutex);
      frame_state_->front_gpu_handle = nullptr;
    }
    shared_textures_[0].Reset();
    shared_textures_[1].Reset();
    shared_handles_[0] = nullptr;
    shared_handles_[1] = nullptr;
    texture_registrar_->UnregisterTexture(
        id_to_remove, [keep_alive, keep_textures, keep_state]() {});
  }
}

void WindowsWebViewInstance::EnqueueOrRun(std::function<void()> op,
                                          std::function<void()> on_cancel) {
  if (disposed_) {
    if (on_cancel) {
      on_cancel();
    }
    return;
  }
  if (ready_ && webview_) {
    op();
  } else {
    PendingOp entry;
    entry.execute = std::move(op);
    entry.cancel = std::move(on_cancel);
    pending_ops_.push_back(std::move(entry));
  }
}

void WindowsWebViewInstance::LoadHtmlInternal(const std::string& html,
                                              const std::string& base_url) {
  if (!webview_) {
    return;
  }
  std::string resolved = InjectBaseTagIfNeeded(html, base_url);
  has_pending_custom_req_ = false;

  // `NavigateToString` documents have an opaque origin (`about:blank`), so
  // `document.cookie`, `localStorage`, `fetch` and same-origin frames all fail
  // with `SecurityError`. When the caller supplies an http(s) base URL, serve
  // the HTML *as* that URL so the document gets the base URL's real origin
  // (this matches `WKWebView.loadHTMLString(baseURL:)` and Android).
  const bool has_http_base = base_url.rfind("http://", 0) == 0 ||
                             base_url.rfind("https://", 0) == 0;
  if (!has_http_base && resolved.size() <= kMaxInlineNavigateToStringBytes) {
    large_html_utf8_.clear();
    large_html_virtual_uri_.clear();
    const std::wstring whtml = Utf8ToWide(resolved);
    webview_->NavigateToString(whtml.c_str());
    return;
  }

  std::string virtual_uri;
  if (has_http_base) {
    virtual_uri = base_url;
  } else {
    ++large_html_seq_;
    virtual_uri = "http://webview-core.invalid/__inline_" +
                  std::to_string(large_html_seq_) + ".html";
  }
  large_html_utf8_ = std::move(resolved);
  large_html_virtual_uri_ = virtual_uri;
  if (!has_http_base) {
    approved_navigation_uri_ = large_html_virtual_uri_;
  }
  const std::wstring wuri = Utf8ToWide(large_html_virtual_uri_);
  if (has_http_base && wuri != html_filter_pattern_) {
    if (!html_filter_pattern_.empty()) {
      webview_->RemoveWebResourceRequestedFilter(
          html_filter_pattern_.c_str(),
          COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT);
    }
    webview_->AddWebResourceRequestedFilter(
        wuri.c_str(), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT);
    html_filter_pattern_ = wuri;
  }
  webview_->Navigate(wuri.c_str());
}

void WindowsWebViewInstance::ApplyBackgroundColor(uint32_t argb) {
  bg_color_argb_ = argb;
  if (!controller_) {
    return;
  }
  ComPtr<ICoreWebView2Controller2> ctrl2;
  if (SUCCEEDED(controller_.As(&ctrl2)) && ctrl2) {
    const BYTE a = static_cast<BYTE>((argb >> 24) & 0xFF);
    COREWEBVIEW2_COLOR c{};
    if (a == 0) {
      c.A = 0;
      c.R = 0;
      c.G = 0;
      c.B = 0;
    } else {
      c.A = 255;
      c.R = static_cast<BYTE>((argb >> 16) & 0xFF);
      c.G = static_cast<BYTE>((argb >> 8) & 0xFF);
      c.B = static_cast<BYTE>(argb & 0xFF);
    }
    ctrl2->put_DefaultBackgroundColor(c);
  }
}

void WindowsWebViewInstance::SetSizeInternal(int width,
                                             int height,
                                             double scale_factor) {
  width_ = width > 0 ? width : 1;
  height_ = height > 0 ? height : 1;
  scale_factor_ = scale_factor > 0.0 ? scale_factor : 1.0;

  if (root_visual_) {
    ABI::Windows::Foundation::Numerics::Vector2 sz{};
    sz.X = static_cast<float>(width_);
    sz.Y = static_cast<float>(height_);
    root_visual_->put_Size(sz);
  }
  if (frame_pool_ && gfx_ != nullptr && gfx_->winrt_d3d_device() != nullptr) {
    ABI::Windows::Graphics::SizeInt32 pool_sz{};
    pool_sz.Width = width_;
    pool_sz.Height = height_;
    frame_pool_->Recreate(
        gfx_->winrt_d3d_device(),
        ABI::Windows::Graphics::DirectX::
            DirectXPixelFormat_B8G8R8A8UIntNormalized,
        2, pool_sz);
  }
  if (controller_) {
    ComPtr<ICoreWebView2Controller3> ctrl3;
    if (SUCCEEDED(controller_.As(&ctrl3)) && ctrl3) {
      ctrl3->put_RasterizationScale(scale_factor_);
    }
    RECT bounds{};
    bounds.left = 0;
    bounds.top = 0;
    bounds.right = static_cast<LONG>(width_);
    bounds.bottom = static_cast<LONG>(height_);
    controller_->put_Bounds(bounds);
  }
}

void WindowsWebViewInstance::SetVisibilityInternal(bool visible) {
  visible_ = visible;
  if (!visible) {
    CancelActiveContextMenu();
  }
  if (root_visual_) {
    root_visual_->put_IsVisible(visible ? TRUE : FALSE);
  }
  if (controller_) {
    controller_->put_IsVisible(visible ? TRUE : FALSE);
  }
}

void WindowsWebViewInstance::RebuildChannelScript() {
  if (!webview_) {
    return;
  }
  if (!channel_script_id_.empty()) {
    webview_->RemoveScriptToExecuteOnDocumentCreated(
        channel_script_id_.c_str());
    channel_script_id_.clear();
  }
  if (channel_names_.empty()) {
    return;
  }
  std::string js;
  for (const auto& name : channel_names_) {
    const std::string quoted = EscapeJsonString(name);
    js += "window[" + quoted + "]={postMessage:function(m){"
          "window.chrome.webview.postMessage(\"\\x01__wvcore__:\"+" +
          quoted + "+\"\\n\"+String(m));}};";
  }
  const std::wstring wjs = Utf8ToWide(js);
  std::weak_ptr<WindowsWebViewInstance> weak_self = shared_from_this();
  webview_->AddScriptToExecuteOnDocumentCreated(
      wjs.c_str(),
      Callback<ICoreWebView2AddScriptToExecuteOnDocumentCreatedCompletedHandler>(
          [weak_self](HRESULT hr, LPCWSTR id) -> HRESULT {
            if (auto self = weak_self.lock()) {
              if (SUCCEEDED(hr) && id != nullptr) {
                self->channel_script_id_ = id;
              }
            }
            return S_OK;
          })
          .Get());
}

void WindowsWebViewInstance::ReplayCustomRequest(const std::string& url) {
  if (!webview_) {
    return;
  }
  const std::wstring wurl = Utf8ToWide(url);
  ComPtr<ICoreWebView2_2> webview2;
  ComPtr<ICoreWebView2Environment2> env2;
  if (env3_ && SUCCEEDED(env3_.As(&env2)) && env2 &&
      SUCCEEDED(webview_.As(&webview2)) && webview2) {
    ComPtr<IStream> body_stream;
    if (!pending_custom_req_body_.empty()) {
      body_stream.Attach(
          SHCreateMemStream(pending_custom_req_body_.data(),
                            static_cast<UINT>(pending_custom_req_body_.size())));
    }
    const std::wstring wmethod = Utf8ToWide(
        pending_custom_req_method_.empty() ? "GET"
                                           : pending_custom_req_method_);
    const std::wstring wheaders = Utf8ToWide(pending_custom_req_headers_);
    ComPtr<ICoreWebView2WebResourceRequest> req;
    if (SUCCEEDED(env2->CreateWebResourceRequest(
            wurl.c_str(), wmethod.c_str(), body_stream.Get(), wheaders.c_str(),
            &req)) &&
        req) {
      has_pending_custom_req_ = false;
      webview2->NavigateWithWebResourceRequest(req.Get());
      return;
    }
  }
  has_pending_custom_req_ = false;
  webview_->Navigate(wurl.c_str());
}

void WindowsWebViewInstance::LoadRequestInternal(
    const std::string& url,
    const std::string& method,
    const flutter::EncodableValue* headers_val,
    const flutter::EncodableValue* body_val) {
  if (!webview_) {
    return;
  }
  large_html_utf8_.clear();
  large_html_virtual_uri_.clear();

  pending_custom_req_uri_ = url;
  pending_custom_req_method_ = method.empty() ? "GET" : method;
  pending_custom_req_headers_.clear();
  pending_custom_req_body_.clear();

  if (body_val != nullptr) {
    if (const auto* bytes = std::get_if<std::vector<uint8_t>>(body_val)) {
      pending_custom_req_body_ = *bytes;
    }
  }
  if (headers_val != nullptr) {
    if (const auto* hmap = std::get_if<flutter::EncodableMap>(headers_val)) {
      for (const auto& kv : *hmap) {
        const auto* k = std::get_if<std::string>(&kv.first);
        const auto* v = std::get_if<std::string>(&kv.second);
        if (k != nullptr && v != nullptr) {
          pending_custom_req_headers_ += *k + ": " + *v + "\r\n";
        }
      }
    }
  }

  has_pending_custom_req_ = (pending_custom_req_method_ != "GET") ||
                            !pending_custom_req_headers_.empty() ||
                            !pending_custom_req_body_.empty();
  if (has_pending_custom_req_) {
    ReplayCustomRequest(url);
    has_pending_custom_req_ = (pending_custom_req_method_ != "GET") ||
                              !pending_custom_req_headers_.empty() ||
                              !pending_custom_req_body_.empty();
    return;
  }

  const std::wstring wurl = Utf8ToWide(url);
  webview_->Navigate(wurl.c_str());
}

void WindowsWebViewInstance::CancelActiveContextMenu() {
  if (!showing_context_menu_) {
    return;
  }
  ::EndMenu();
  const HWND menu_owner =
      (top_hwnd_ != nullptr && ::IsWindow(top_hwnd_)) ? top_hwnd_
                                                      : parent_hwnd_;
  if (menu_owner != nullptr && ::IsWindow(menu_owner)) {
    ::SendMessageW(menu_owner, WM_CANCELMODE, 0, 0);
  }
}

HRESULT WindowsWebViewInstance::HandleContextMenuRequested(
    ICoreWebView2ContextMenuRequestedEventArgs* args) {
  if (args == nullptr) {
    return S_OK;
  }
  if (disposed_ || !visible_ || parent_hwnd_ == nullptr) {
    args->put_Handled(TRUE);
    return S_OK;
  }
  ComPtr<ICoreWebView2ContextMenuItemCollection> items;
  if (FAILED(args->get_MenuItems(&items)) || !items) {
    args->put_Handled(TRUE);
    return S_OK;
  }
  UINT32 count = 0;
  if (FAILED(items->get_Count(&count)) || count == 0) {
    args->put_Handled(TRUE);
    return S_OK;
  }

  POINT menu_pt{};
  if (::GetCursorPos(&menu_pt) == FALSE) {
    if (has_last_context_screen_pt_) {
      menu_pt = last_context_screen_pt_;
    } else if (::IsWindow(parent_hwnd_)) {
      RECT rc{};
      if (::GetWindowRect(parent_hwnd_, &rc) != FALSE) {
        menu_pt.x = rc.left + (rc.right - rc.left) / 2;
        menu_pt.y = rc.top + (rc.bottom - rc.top) / 2;
      }
    }
  }

  // `TrackPopupMenu` runs a modal message loop. Doing that inside the event
  // handler stalls WebView2 event delivery (every `ExecuteScript` completion
  // and channel round trip hangs until the menu closes). Take a deferral and
  // show the menu from a later message-loop turn instead.
  ComPtr<ICoreWebView2Deferral> deferral;
  if (FAILED(args->GetDeferral(&deferral)) || !deferral) {
    args->put_Handled(TRUE);
    return S_OK;
  }
  ComPtr<ICoreWebView2ContextMenuRequestedEventArgs> args_ref(args);
  std::weak_ptr<WindowsWebViewInstance> weak_self = shared_from_this();
  const bool queued = gfx_ != nullptr && gfx_->PostToUiThread(
      [weak_self, args_ref, items, deferral, menu_pt]() {
        if (auto inst = weak_self.lock()) {
          if (!inst->disposed_ && inst->visible_) {
            inst->ShowContextMenu(args_ref.Get(), items.Get(), menu_pt);
          } else {
            args_ref->put_Handled(TRUE);
          }
        } else {
          args_ref->put_Handled(TRUE);
        }
        deferral->Complete();
      });
  if (!queued) {
    args->put_Handled(TRUE);
    deferral->Complete();
  }
  return S_OK;
}

void WindowsWebViewInstance::ShowContextMenu(
    ICoreWebView2ContextMenuRequestedEventArgs* args,
    ICoreWebView2ContextMenuItemCollection* items,
    POINT screen_pt) {
  if (args == nullptr) {
    return;
  }
  // Always mark handled so WebView2 never falls back to anchoring its own menu
  // against the off-client (-32000, -32000) composition host HWND (which
  // Windows clamps to monitor (0, 0)).
  args->put_Handled(TRUE);

  const HWND menu_owner =
      (top_hwnd_ != nullptr && ::IsWindow(top_hwnd_)) ? top_hwnd_
                                                      : parent_hwnd_;
  if (items == nullptr || menu_owner == nullptr || !::IsWindow(menu_owner) ||
      !::IsWindowVisible(menu_owner) || ::IsIconic(menu_owner)) {
    return;
  }
  HMENU menu = CreatePopupMenu();
  if (menu == nullptr) {
    return;
  }
  std::map<UINT, INT32> command_ids;
  UINT next_id = 1;
  AppendContextMenuItems(menu, items, &command_ids, &next_id);
  TrimTrailingMenuSeparator(menu);

  if (GetMenuItemCount(menu) > 0) {
    showing_context_menu_ = true;
    SetForegroundWindow(menu_owner);
    const UINT picked = static_cast<UINT>(TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN |
            TPM_RIGHTBUTTON,
        screen_pt.x, screen_pt.y, 0, menu_owner, nullptr));
    showing_context_menu_ = false;
    ::PostMessageW(menu_owner, WM_NULL, 0, 0);
    if (picked != 0 && !disposed_) {
      const auto found = command_ids.find(picked);
      if (found != command_ids.end()) {
        args->put_SelectedCommandId(found->second);
      }
    }
    if (!disposed_ && parent_hwnd_ != nullptr &&
        parent_hwnd_ != ::GetDesktopWindow() && ::IsWindow(parent_hwnd_) &&
        ::GetActiveWindow() == menu_owner && ::GetFocus() != parent_hwnd_) {
      ::SetFocus(parent_hwnd_);
    }
  }
  DestroyMenu(menu);
}

void WindowsWebViewInstance::HandlePointerEvent(
    const flutter::EncodableMap& args) {
  if (!comp_controller_) {
    return;
  }
  const std::string kind = GetStringArg(args, "kind");
  int64_t button = 0;
  int64_t buttons = 0;
  GetIntArg(args, "button", &button);
  GetIntArg(args, "buttons", &buttons);
  if ((kind == "down" || kind == "up") &&
      (button == 2 || (buttons & 2) != 0)) {
    POINT cur{};
    if (::GetCursorPos(&cur) != FALSE) {
      last_context_screen_pt_ = cur;
      has_last_context_screen_pt_ = true;
    }
  }
  const double x = GetDoubleArg(args, "x", 0.0);
  const double y = GetDoubleArg(args, "y", 0.0);
  const double dx = GetDoubleArg(args, "deltaX", 0.0);
  const double dy = GetDoubleArg(args, "deltaY", 0.0);
  const bool shift =
      GetBoolArg(args, "shift", (GetKeyState(VK_SHIFT) & 0x8000) != 0);
  const bool control =
      GetBoolArg(args, "control", (GetKeyState(VK_CONTROL) & 0x8000) != 0);

  COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS vkeys =
      COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE;
  if ((buttons & 1) != 0) {
    vkeys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON;
  }
  if ((buttons & 2) != 0) {
    vkeys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_RIGHT_BUTTON;
  }
  if ((buttons & 4) != 0) {
    vkeys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_MIDDLE_BUTTON;
  }
  if (shift) {
    vkeys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_SHIFT;
  }
  if (control) {
    vkeys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_CONTROL;
  }

  POINT pt{};
  pt.x = static_cast<LONG>(std::lround(x));
  pt.y = static_cast<LONG>(std::lround(y));

  const HWND prev_focus = ::GetFocus();
  const HWND prev_capture = ::GetCapture();

  if (kind == "move") {
    comp_controller_->SendMouseInput(COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE, vkeys,
                                     0, pt);
  } else if (kind == "leave") {
    comp_controller_->SendMouseInput(COREWEBVIEW2_MOUSE_EVENT_KIND_LEAVE,
                                     COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                                     0, pt);
  } else if (kind == "down") {
    COREWEBVIEW2_MOUSE_EVENT_KIND ev =
        COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN;
    if (button == 2) {
      ev = COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN;
    } else if (button == 3) {
      ev = COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOWN;
    }
    comp_controller_->SendMouseInput(ev, vkeys, 0, pt);
  } else if (kind == "up") {
    COREWEBVIEW2_MOUSE_EVENT_KIND ev =
        COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP;
    if (button == 2) {
      ev = COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP;
    } else if (button == 3) {
      ev = COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_UP;
    }
    comp_controller_->SendMouseInput(ev, vkeys, 0, pt);
  } else if (kind == "wheel") {
    if (std::abs(dy) > 0.01) {
      const double clamped_y =
          dy < -32768.0 ? -32768.0 : (dy > 32767.0 ? 32767.0 : dy);
      const auto wheel_y =
          static_cast<UINT32>(static_cast<INT32>(std::lround(clamped_y)));
      comp_controller_->SendMouseInput(COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL,
                                       vkeys, wheel_y, pt);
    }
    if (std::abs(dx) > 0.01) {
      const double clamped_x =
          dx < -32768.0 ? -32768.0 : (dx > 32767.0 ? 32767.0 : dx);
      const auto wheel_x =
          static_cast<UINT32>(static_cast<INT32>(std::lround(clamped_x)));
      comp_controller_->SendMouseInput(
          COREWEBVIEW2_MOUSE_EVENT_KIND_HORIZONTAL_WHEEL, vkeys, wheel_x, pt);
    }
  }

  RestoreFlutterViewInputState(prev_focus, prev_capture);
}

void WindowsWebViewInstance::RestoreFlutterViewInputState(
    HWND prev_focus,
    HWND prev_capture) {
  if (parent_hwnd_ == nullptr || parent_hwnd_ == ::GetDesktopWindow() ||
      !::IsWindow(parent_hwnd_)) {
    return;
  }
  const HWND cur_capture = ::GetCapture();
  if (cur_capture != prev_capture && IsInternalWebViewWindow(cur_capture)) {
    if (prev_capture == parent_hwnd_ && IsAnyMouseButtonPressed()) {
      ::SetCapture(parent_hwnd_);
    } else {
      ::ReleaseCapture();
    }
  }
  const HWND cur_focus = ::GetFocus();
  if (cur_focus != prev_focus && IsInternalWebViewWindow(cur_focus) &&
      prev_focus == parent_hwnd_) {
    ::SetFocus(parent_hwnd_);
  }
}

void WindowsWebViewInstance::HandleKeyEvent(const flutter::EncodableMap& args) {
  if (!webview_ || !controller_) {
    return;
  }
  if (parent_hwnd_ != nullptr && parent_hwnd_ != ::GetDesktopWindow() &&
      ::GetFocus() != parent_hwnd_) {
    const HWND active = ::GetActiveWindow();
    if (active == parent_hwnd_ ||
        (top_hwnd_ != nullptr && active == top_hwnd_)) {
      ::SetFocus(parent_hwnd_);
    } else {
      return;
    }
  }

  const std::string logical_key = GetStringArg(args, "logicalKey");
  bool has_char = false;
  const std::string character = GetStringArg(args, "character", &has_char);
  const bool control = GetBoolArg(args, "control", false);
  const bool shift = GetBoolArg(args, "shift", false);
  const bool alt = GetBoolArg(args, "alt", false);

  std::string js;
  if (control && !alt) {
    if (logical_key == "C" || logical_key == "c") {
      js = "document.execCommand('copy');";
    } else if (logical_key == "X" || logical_key == "x") {
      js = "document.execCommand('cut');";
    } else if (logical_key == "A" || logical_key == "a") {
      js = "document.execCommand('selectAll');";
    } else if (logical_key == "Z" || logical_key == "z") {
      js = shift ? "document.execCommand('redo');"
                 : "document.execCommand('undo');";
    }
  } else if (logical_key == "Arrow Down") {
    js = "window.scrollBy(0, 40);";
  } else if (logical_key == "Arrow Up") {
    js = "window.scrollBy(0, -40);";
  } else if (logical_key == "Page Down") {
    js = "window.scrollBy(0, Math.max(80, window.innerHeight - 40));";
  } else if (logical_key == "Page Up") {
    js = "window.scrollBy(0, -Math.max(80, window.innerHeight - 40));";
  } else if (logical_key == "Home") {
    js = "window.scrollTo(window.scrollX, 0);";
  } else if (logical_key == "End") {
    js = "window.scrollTo(window.scrollX, document.documentElement."
         "scrollHeight || 0);";
  } else if (logical_key == "Backspace") {
    js = "document.execCommand('delete');";
  } else if (logical_key == "Delete") {
    js = "document.execCommand('forwardDelete');";
  } else if (logical_key == "Enter") {
    js = "document.execCommand('insertLineBreak');";
  } else if (has_char && !character.empty() && !control && !alt) {
    js = "document.execCommand('insertText', false, " +
         EscapeJsonString(character) + ");";
  }

  if (!js.empty()) {
    const std::wstring wjs = Utf8ToWide(js);
    webview_->ExecuteScript(wjs.c_str(), nullptr);
  }
}

void WindowsWebViewInstance::HandleCall(
    const std::string& method,
    const flutter::EncodableMap& args,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
  if (disposed_) {
    result->Success();
    return;
  }

  if (method == "setSize") {
    int64_t w = 1;
    int64_t h = 1;
    GetIntArg(args, "width", &w);
    GetIntArg(args, "height", &h);
    const double scale = GetDoubleArg(args, "scaleFactor", 1.0);
    SetSizeInternal(static_cast<int>(w), static_cast<int>(h), scale);
    result->Success();
    return;
  }

  if (method == "setVisibility") {
    SetVisibilityInternal(GetBoolArg(args, "visible", true));
    result->Success();
    return;
  }

  if (method == "pointerEvent") {
    HandlePointerEvent(args);
    result->Success();
    return;
  }

  if (method == "keyEvent") {
    HandleKeyEvent(args);
    result->Success();
    return;
  }

  if (method == "setFocus") {
    // Keep Win32 HWND focus on `FLUTTERVIEW` (`parent_hwnd_`) at all times so
    // Flutter continues to receive all pointer and keyboard messages. Calling
    // `controller_->MoveFocus` moves Win32 focus into WebView2's internal child
    // HWND, breaking Flutter input delivery; in-page DOM focus is driven by
    // `SendMouseInput` clicks instead.
    (void)args;
    if (parent_hwnd_ != nullptr && parent_hwnd_ != ::GetDesktopWindow() &&
        ::IsWindow(parent_hwnd_) && ::GetFocus() != parent_hwnd_) {
      const HWND active = ::GetActiveWindow();
      if (active == parent_hwnd_ ||
          (top_hwnd_ != nullptr && active == top_hwnd_)) {
        ::SetFocus(parent_hwnd_);
      }
    }
    result->Success();
    return;
  }

  if (method == "loadHtmlString") {
    const std::string html = GetStringArg(args, "html");
    const std::string base_url = GetStringArg(args, "baseUrl");
    EnqueueOrRun([this, html, base_url]() { LoadHtmlInternal(html, base_url); });
    result->Success();
    return;
  }

  if (method == "loadRequest") {
    const std::string url = GetStringArg(args, "url");
    const std::string req_method = GetStringArg(args, "method");
    flutter::EncodableMap copied = args;
    EnqueueOrRun([this, url, req_method, copied]() {
      LoadRequestInternal(url, req_method, LookupMap(copied, "headers"),
                          LookupMap(copied, "body"));
    });
    result->Success();
    return;
  }

  if (method == "loadFile") {
    std::string path = GetStringArg(args, "path");
    for (size_t i = 0; i < path.size(); ++i) {
      if (path[i] == '\\') {
        path[i] = '/';
      }
    }
    const std::string file_url =
        path.rfind("file://", 0) == 0 ? path : ("file:///" + path);
    EnqueueOrRun([this, file_url]() {
      large_html_utf8_.clear();
      large_html_virtual_uri_.clear();
      has_pending_custom_req_ = false;
      const std::wstring wurl = Utf8ToWide(file_url);
      webview_->Navigate(wurl.c_str());
    });
    result->Success();
    return;
  }

  if (method == "loadFlutterAsset") {
    result->Success(flutter::EncodableValue(false));
    return;
  }

  if (method == "setJavaScriptEnabled") {
    js_enabled_ = GetBoolArg(args, "enabled", true);
    EnqueueOrRun([this]() {
      ComPtr<ICoreWebView2Settings> settings;
      if (SUCCEEDED(webview_->get_Settings(&settings)) && settings) {
        settings->put_IsScriptEnabled(js_enabled_ ? TRUE : FALSE);
      }
    });
    result->Success();
    return;
  }

  if (method == "setBackgroundColor") {
    int64_t raw = 0;
    GetIntArg(args, "color", &raw);
    ApplyBackgroundColor(static_cast<uint32_t>(raw));
    result->Success();
    return;
  }

  if (method == "setUserAgent") {
    bool has_ua = false;
    const std::string ua = GetStringArg(args, "userAgent", &has_ua);
    custom_user_agent_ = (has_ua && !ua.empty()) ? Utf8ToWide(ua) : L"";
    EnqueueOrRun([this]() {
      ComPtr<ICoreWebView2Settings> settings;
      if (SUCCEEDED(webview_->get_Settings(&settings)) && settings) {
        ComPtr<ICoreWebView2Settings2> settings2;
        if (SUCCEEDED(settings.As(&settings2)) && settings2) {
          settings2->put_UserAgent(custom_user_agent_.empty()
                                       ? default_user_agent_.c_str()
                                       : custom_user_agent_.c_str());
        }
      }
    });
    result->Success();
    return;
  }

  if (method == "enableZoom") {
    zoom_enabled_ = GetBoolArg(args, "enabled", true);
    EnqueueOrRun([this]() {
      ComPtr<ICoreWebView2Settings> settings;
      if (SUCCEEDED(webview_->get_Settings(&settings)) && settings) {
        settings->put_IsZoomControlEnabled(zoom_enabled_ ? TRUE : FALSE);
      }
    });
    result->Success();
    return;
  }

  if (method == "setHasNavigationDecision") {
    has_navigation_decision_ = GetBoolArg(args, "enabled", false);
    result->Success();
    return;
  }

  if (method == "runJavaScript") {
    const std::string script = GetStringArg(args, "script");
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, script, shared_res]() {
          const std::wstring wscript = Utf8ToWide(script);
          webview_->ExecuteScript(
              wscript.c_str(),
              Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
                  [shared_res](HRESULT /*hr*/, LPCWSTR /*json*/) -> HRESULT {
                    shared_res->Success();
                    return S_OK;
                  })
                  .Get());
        },
        [shared_res]() { shared_res->Success(); });
    return;
  }

  if (method == "runJavaScriptReturningResult") {
    const std::string script = GetStringArg(args, "script");
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, script, shared_res]() {
          const std::wstring wscript = Utf8ToWide(script);
          webview_->ExecuteScript(
              wscript.c_str(),
              Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
                  [shared_res](HRESULT hr, LPCWSTR json) -> HRESULT {
                    if (FAILED(hr)) {
                      shared_res->Error("JS_ERROR", "ExecuteScript failed");
                      return S_OK;
                    }
                    shared_res->Success(DecodeJsonScalar(WideToUtf8(json)));
                    return S_OK;
                  })
                  .Get());
        },
        [shared_res]() { shared_res->Success(flutter::EncodableValue()); });
    return;
  }

  if (method == "addJavaScriptChannel") {
    const std::string name = GetStringArg(args, "name");
    if (!name.empty()) {
      channel_names_.insert(name);
      EnqueueOrRun([this]() { RebuildChannelScript(); });
    }
    result->Success();
    return;
  }

  if (method == "removeJavaScriptChannel") {
    const std::string name = GetStringArg(args, "name");
    if (!name.empty() && channel_names_.erase(name) > 0) {
      EnqueueOrRun([this]() { RebuildChannelScript(); });
    }
    result->Success();
    return;
  }

  if (method == "reload") {
    EnqueueOrRun([this]() { webview_->Reload(); });
    result->Success();
    return;
  }

  if (method == "stopLoading") {
    EnqueueOrRun([this]() { webview_->Stop(); });
    result->Success();
    return;
  }

  if (method == "canGoBack") {
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, shared_res]() {
          BOOL can = FALSE;
          webview_->get_CanGoBack(&can);
          shared_res->Success(flutter::EncodableValue(can != FALSE));
        },
        [shared_res]() {
          shared_res->Success(flutter::EncodableValue(false));
        });
    return;
  }

  if (method == "canGoForward") {
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, shared_res]() {
          BOOL can = FALSE;
          webview_->get_CanGoForward(&can);
          shared_res->Success(flutter::EncodableValue(can != FALSE));
        },
        [shared_res]() {
          shared_res->Success(flutter::EncodableValue(false));
        });
    return;
  }

  if (method == "goBack") {
    EnqueueOrRun([this]() {
      BOOL can = FALSE;
      if (SUCCEEDED(webview_->get_CanGoBack(&can)) && can) {
        webview_->GoBack();
      }
    });
    result->Success();
    return;
  }

  if (method == "goForward") {
    EnqueueOrRun([this]() {
      BOOL can = FALSE;
      if (SUCCEEDED(webview_->get_CanGoForward(&can)) && can) {
        webview_->GoForward();
      }
    });
    result->Success();
    return;
  }

  if (method == "currentUrl") {
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, shared_res]() {
          LPWSTR uri_w = nullptr;
          webview_->get_Source(&uri_w);
          std::string uri = WideToUtf8(uri_w);
          if (uri_w != nullptr) {
            CoTaskMemFree(uri_w);
          }
          if (!large_html_virtual_uri_.empty() &&
              uri == large_html_virtual_uri_) {
            uri = "about:blank";
          }
          shared_res->Success(uri.empty() ? flutter::EncodableValue()
                                          : flutter::EncodableValue(uri));
        },
        [shared_res]() { shared_res->Success(flutter::EncodableValue()); });
    return;
  }

  if (method == "getTitle") {
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, shared_res]() {
          LPWSTR title_w = nullptr;
          webview_->get_DocumentTitle(&title_w);
          const std::string title = WideToUtf8(title_w);
          if (title_w != nullptr) {
            CoTaskMemFree(title_w);
          }
          shared_res->Success(flutter::EncodableValue(title));
        },
        [shared_res]() { shared_res->Success(flutter::EncodableValue()); });
    return;
  }

  if (method == "scrollTo") {
    int64_t x = 0;
    int64_t y = 0;
    GetIntArg(args, "x", &x);
    GetIntArg(args, "y", &y);
    char buf[96];
    snprintf(buf, sizeof(buf), "window.scrollTo(%lld, %lld);",
             static_cast<long long>(x), static_cast<long long>(y));
    const std::wstring wscript = Utf8ToWide(buf);
    EnqueueOrRun([this, wscript]() {
      webview_->ExecuteScript(wscript.c_str(), nullptr);
    });
    result->Success();
    return;
  }

  if (method == "scrollBy") {
    int64_t x = 0;
    int64_t y = 0;
    GetIntArg(args, "x", &x);
    GetIntArg(args, "y", &y);
    char buf[96];
    snprintf(buf, sizeof(buf), "window.scrollBy(%lld, %lld);",
             static_cast<long long>(x), static_cast<long long>(y));
    const std::wstring wscript = Utf8ToWide(buf);
    EnqueueOrRun([this, wscript]() {
      webview_->ExecuteScript(wscript.c_str(), nullptr);
    });
    result->Success();
    return;
  }

  if (method == "getScrollPosition") {
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, shared_res]() {
          webview_->ExecuteScript(
              L"JSON.stringify([window.scrollX||0,window.scrollY||0])",
              Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
                  [shared_res](HRESULT hr, LPCWSTR json) -> HRESULT {
                    double sx = 0.0;
                    double sy = 0.0;
                    if (SUCCEEDED(hr) && json != nullptr) {
                      const flutter::EncodableValue decoded =
                          DecodeJsonScalar(WideToUtf8(json));
                      if (const auto* s = std::get_if<std::string>(&decoded)) {
                        const size_t comma = s->find(',');
                        if (s->size() >= 3 && s->front() == '[' &&
                            comma != std::string::npos) {
                          sx = std::strtod(s->c_str() + 1, nullptr);
                          sy = std::strtod(s->c_str() + comma + 1, nullptr);
                        }
                      }
                    }
                    flutter::EncodableMap pos;
                    pos[flutter::EncodableValue("x")] =
                        flutter::EncodableValue(sx);
                    pos[flutter::EncodableValue("y")] =
                        flutter::EncodableValue(sy);
                    shared_res->Success(
                        flutter::EncodableValue(std::move(pos)));
                    return S_OK;
                  })
                  .Get());
        },
        [shared_res]() {
          flutter::EncodableMap pos;
          pos[flutter::EncodableValue("x")] = flutter::EncodableValue(0.0);
          pos[flutter::EncodableValue("y")] = flutter::EncodableValue(0.0);
          shared_res->Success(flutter::EncodableValue(std::move(pos)));
        });
    return;
  }

  if (method == "clearCache") {
    EnqueueOrRun([this]() {
      webview_->CallDevToolsProtocolMethod(L"Network.clearBrowserCache", L"{}",
                                           nullptr);
    });
    result->Success();
    return;
  }

  if (method == "clearCookies") {
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, shared_res]() {
          webview_->CallDevToolsProtocolMethod(
              L"Network.clearBrowserCookies", L"{}",
              Callback<
                  ICoreWebView2CallDevToolsProtocolMethodCompletedHandler>(
                  [shared_res](HRESULT hr, LPCWSTR /*json*/) -> HRESULT {
                    shared_res->Success(
                        flutter::EncodableValue(SUCCEEDED(hr)));
                    return S_OK;
                  })
                  .Get());
        },
        [shared_res]() {
          shared_res->Success(flutter::EncodableValue(false));
        });
    return;
  }

  if (method == "setCookie") {
    const std::string name = GetStringArg(args, "name");
    const std::string value = GetStringArg(args, "value");
    const std::string domain = GetStringArg(args, "domain");
    std::string path = GetStringArg(args, "path");
    if (path.empty()) {
      path = "/";
    }
    if (name.empty() || domain.empty()) {
      result->Success();
      return;
    }
    const std::string params_json =
        "{\"name\":" + EscapeJsonString(name) +
        ",\"value\":" + EscapeJsonString(value) +
        ",\"domain\":" + EscapeJsonString(domain) +
        ",\"path\":" + EscapeJsonString(path) + "}";
    const std::wstring wparams = Utf8ToWide(params_json);
    auto shared_res =
        std::shared_ptr<flutter::MethodResult<flutter::EncodableValue>>(
            std::move(result));
    EnqueueOrRun(
        [this, wparams, shared_res]() {
          webview_->CallDevToolsProtocolMethod(
              L"Network.setCookie", wparams.c_str(),
              Callback<
                  ICoreWebView2CallDevToolsProtocolMethodCompletedHandler>(
                  [shared_res](HRESULT /*hr*/, LPCWSTR /*json*/) -> HRESULT {
                    shared_res->Success();
                    return S_OK;
                  })
                  .Get());
        },
        [shared_res]() { shared_res->Success(); });
    return;
  }

  if (method == "clearLocalStorage") {
    EnqueueOrRun([this]() {
      webview_->ExecuteScript(L"window.localStorage.clear();", nullptr);
    });
    result->Success();
    return;
  }

  result->NotImplemented();
}

}  // namespace

void WebviewCorePluginCApiRegisterWithRegistrar(
    FlutterDesktopPluginRegistrarRef registrar) {
  WebviewCorePlugin::RegisterWithRegistrar(
      flutter::PluginRegistrarManager::GetInstance()
          ->GetRegistrar<flutter::PluginRegistrarWindows>(registrar));
}
