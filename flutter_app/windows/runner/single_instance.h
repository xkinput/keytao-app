#ifndef RUNNER_SINGLE_INSTANCE_H_
#define RUNNER_SINGLE_INSTANCE_H_

#include <windows.h>

inline constexpr wchar_t kKeyTaoWindowClass[] = L"ink.rea.keytao-app.FlutterWindow";
inline constexpr wchar_t kKeyTaoWindowReady[] = L"ink.rea.keytao-app.ArgsReady";
inline constexpr ULONG_PTR kKeyTaoArgsMessage = 0x4B544131;
inline constexpr UINT kKeyTaoFirstFrame = WM_APP + 1;

// Own the mutex until the runner and Flutter engine have been destroyed.
class SingleInstance {
 public:
  enum class Result { primary, forwarded, failed };
  SingleInstance() = default;
  ~SingleInstance();
  SingleInstance(const SingleInstance&) = delete;
  SingleInstance& operator=(const SingleInstance&) = delete;
  Result AcquireOrForward();

 private:
  HANDLE mutex_ = nullptr;
  bool owns_mutex_ = false;
};

#endif  // RUNNER_SINGLE_INSTANCE_H_
