#include <flutter/dart_project.h>
#include <flutter/flutter_engine.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <algorithm>

#include "flutter_window.h"
#include "single_instance.h"
#include "utils.h"

static int RunCommand(const flutter::DartProject& project) {
  flutter::FlutterEngine engine(project);
  if (!engine.Run()) return EXIT_FAILURE;
  MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0) > 0) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }
  return EXIT_FAILURE;
}

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  std::vector<std::string> command_line_arguments = GetCommandLineArguments();
  // Command invocations need their own completion and exit code.
  const bool command = std::any_of(
      command_line_arguments.begin(), command_line_arguments.end(),
      [](const std::string& arg) {
        return arg == "--deploy" || arg == "--disable-engine" ||
               arg == "--enable-engine";
      });
  SingleInstance single_instance;
  if (!command) {
    const auto launch = single_instance.AcquireOrForward();
    if (launch != SingleInstance::Result::primary) {
      return launch == SingleInstance::Result::forwarded ? EXIT_SUCCESS : EXIT_FAILURE;
    }
  }
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }
  if (command) FlutterDesktopResyncOutputStreams();

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  if (command) {
    const int result = RunCommand(project);
    ::CoUninitialize();
    return result;
  }

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(800, 700);
  if (!window.Create(L"KeyTao", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0) > 0) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}
