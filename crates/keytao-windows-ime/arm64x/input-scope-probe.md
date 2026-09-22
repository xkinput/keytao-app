# Manual RichEdit input-scope probe

**Run the native-control probe only in a VM or disposable account.** It is not
an automated isolated regression test. RichEdit/TSF can load and activate an
installed TIP despite the initial `NOACTIVATETIP` flag. Module assertions detect
that loading afterward; they cannot prevent a TIP's initialization or undo its
effects. An explicit `--manual` argument is required before COM or native
controls are initialized. `--enum PID` is a separate read-only operation.

`input-scope-probe.cpp` records the raw HRESULTs from a native RichEdit TSF
context: `GetAppProperty`, `GetSelection`, `GetValue`, `QueryInterface` and
`GetInputScopes`. It also records the active view HWND/class, password style,
password character and context status flags. It never reads document text.

Build from an x64 Visual Studio Developer PowerShell at the repository root;
run the resulting helper only inside that disposable environment:

```powershell
New-Item -ItemType Directory -Force target/input-scope-probe | Out-Null
cl /nologo /EHsc /W4 /std:c++17 crates/keytao-windows-ime/arm64x/input-scope-probe.cpp /Fe:target/input-scope-probe/probe.exe /Fo:target/input-scope-probe/probe.obj /link user32.lib advapi32.lib ole32.lib oleaut32.lib uuid.lib
./target/input-scope-probe/probe.exe --manual | Tee-Object target/input-scope-probe/system.txt
```

The default DLL is the system `msftedit.dll`. Pass an absolute path to a
trusted, matching-architecture Microsoft RichEdit DLL to compare another
version. A readable Notepad package's `Notepad/riched20.dll` can be copied into
`target` and loaded there when direct loading from WindowsApps is denied. Do
not change package ACLs, take ownership, redistribute the DLL, or add it to Git.
Verify that the copied DLL's SHA-256 matches its source.

```powershell
./target/input-scope-probe/probe.exe --manual 'D:/path/to/isolated/riched20.dll' | Tee-Object target/input-scope-probe/notepad.txt
./target/input-scope-probe/probe.exe --enum 12345
```

`--enum` reads only HWND, PID, TID and window class metadata for the specified
process. It does not obtain text, send messages, inject code or activate a
window. The ordinary probe creates hidden windows in its own process and
requests synchronous **read** edit sessions. It activates only its local thread
manager with `TF_TMAE_NOACTIVATETIP | TF_TMAE_NOACTIVATEKEYBOARDLAYOUT`. Those
flags suppress activation **during that call only**; TSF may activate installed
TIPs asynchronously afterward, as documented by
[Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfthreadmgrex-activateex).
The helper therefore does **not** pump/translate/dispatch messages: RichEdit
publishes its context synchronously. Do not add a message pump, even while
waiting or cleaning up.

After each phase it checks its actual loaded modules against Rime, KeyTao and
the in-process DLLs of TIPs registered under HKLM/HKCU `SOFTWARE/Microsoft/CTF/TIP`.
It also checks that its thread's real focus and keyboard layout are unchanged.
Any failed inspection or unexpected mapping terminates this helper with exit
code 8. A successful result must contain `isolation ... registered_tip_loaded=false`
through the final `cleanup` phase. These snapshots describe mapped modules at
the time of each check; neither the checks nor the flags guarantee that no TIP
was ever loaded or executed. Removing the pump reduces one activation trigger,
but is not a security boundary or permission to run on a user's normal desktop.
The probe does not register/select an IME, send keyboard input, show a window,
or deliberately load a TIP. Keep the checks and hidden-window behavior intact.

The scenarios cover ordinary and empty controls, an actual native password
control (`ES_PASSWORD` plus `EM_SETPASSWORDCHAR`), and the Win32 `SetInputScope`
API. Each probes the collapsed selection, a one-character extension and the
document start. `SetInputScope(IS_PASSWORD)` is **not** a valid substitute for
the native-password scenario: TSF-aware text stores supply their own scope
attributes, and RichEdit can ignore that separate window association.
[Microsoft's ITfInputScope contract](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/nn-inputscope-itfinputscope)
describes that distinction.

Observed on Windows with Notepad 11.2607.14.0 / RichEdit 16.0.20330.42276:

| Control | `GetAppProperty` / selection | `GetValue` | Input scopes |
| --- | --- | --- | --- |
| Normal `RichEditD2DPT` or system `RICHEDIT50W` | `S_OK`, non-null | `E_FAIL` (`0x80004005`) | unavailable |
| Native password control | `S_OK`, non-null | `S_OK`, `VT_UNKNOWN` | `IS_PASSWORD` (`31`) |

Extending the range does not change the ordinary-control failure. Its output
VARIANT remains `VT_EMPTY`, but a failed HRESULT must not be interpreted as a
successful empty property. These observations explain the narrow native
RichEdit compatibility path in `input_context.rs`; they do not justify
accepting arbitrary property failures from other hosts.

Exit code zero means the harness reached at least one edit session. API errors
under investigation remain in the report; this probe is diagnostic, not an
assertion that every host supports a particular result. Rust regression tests
exercise KeyTao's interpretation separately.

Earlier local experiments with a message pump established the raw RichEdit
property behavior but were not sufficient to prove no TIP was loaded. The later
Rust fixture detected Rime mapped despite the activation flags, including in a
repeat with no message pump (startup unmapped, mapped at the end). The revised
C++ diagnostic has only been compiled; it has not been rerun on that user's
desktop after this limitation was identified. Keep subsequent reproduction
manual and confined to a disposable environment.
