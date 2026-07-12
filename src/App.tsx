import { useState, useEffect, useRef } from "react"
import { invoke } from "@tauri-apps/api/core"
import { getVersion } from "@tauri-apps/api/app"
import { listen } from "@tauri-apps/api/event"
import { platform } from "@tauri-apps/plugin-os"
import { openPath, openUrl } from "@tauri-apps/plugin-opener"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { Slider } from "@/components/ui/slider"
import { ColorPicker } from "@/components/ui/color-picker"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog"
import { cn } from "@/lib/utils"
import VirtualLogViewer from "@/components/VirtualLogViewer"
import AndroidImeOnboarding, { type AndroidImeStatus, type AndroidStoragePermissionStatus } from "@/components/AndroidImeOnboarding"
import {
  FolderOpen,
  BookOpen,
  Download,
  CheckCircle2,
  AlertTriangle,
  ExternalLink,
  RefreshCw,
  FileText,
  Folder,
  Info,
  Settings,
  Cpu,
  ScrollText,
  Play,
  Loader2,
  XCircle,
  Keyboard,
  Monitor,
  Sun,
  Moon,
  Palette,
  Paintbrush,
  Columns3,
  Rows3,
  Vibrate,
  VibrateOff,
  SlidersHorizontal,
  User,
  LogIn,
  LogOut,
  Cloud,
  type LucideIcon,
} from "lucide-react"
import DebugTab from "@/components/DebugTab"

type OSType = "windows" | "macos" | "linux" | "android" | "ios" | "unknown"
type Tab = "ime" | "extension" | "about" | "debug"
type ImeUiColorScheme = "auto" | "light" | "dark"
type ImeEffectiveColorScheme = "light" | "dark"
type ImeCandidateOrientation = "horizontal" | "vertical"
type EnterKeyBehavior = "system" | "newline"

const GITHUB_REPOSITORY_URL = "https://github.com/xkinput/keytao-app"
const RIME_DICT_MANAGER_URL_SCHEME = "rime-dict"
const DESKTOP_RIME_DICT_MANAGER_PLATFORMS: OSType[] = ["windows", "macos", "linux"]
const DEFAULT_IME_ACCENT_COLOR = "#3B73D9"
const CROSS_PLATFORM_IME_ACCENT_PRESETS = ["#3B73D9", "#0F9F8F", "#D87A32", "#8B5CF6"]
const ANDROID_STORAGE_PERMISSION_MESSAGE = "请授予 KeyTao 文件访问权限后安装键道方案"
const AUTH_TOKEN_STORAGE_KEY = "keytao.auth.token"
const AUTH_USER_STORAGE_KEY = "keytao.auth.user"

type SchemeKey = "keytao" | "xmjd" | "txjx" | "keydo"

const SCHEME_OPTIONS: Array<{ key: SchemeKey; label: string; asset: string }> = [
  { key: "keytao", label: "键道6", asset: "keytao-linux" },
  { key: "xmjd", label: "星猫键道", asset: "xmjd6.zip" },
  { key: "txjx", label: "天行键", asset: "txjx.zip" },
  { key: "keydo", label: "键道·我流", asset: "nightly zip" },
]

function schemeKeyFromSchemas(schemas: string[]): SchemeKey | null {
  for (const schema of schemas) {
    const normalized = schema.trim().toLowerCase()
    if (normalized.startsWith("xmjd6")) return "xmjd"
    if (normalized.startsWith("txjx")) return "txjx"
    if (normalized.startsWith("keydo")) return "keydo"
    if (normalized.startsWith("keytao")) return "keytao"
  }
  return null
}

function hasSystemIme(os: OSType): boolean {
  return os === "linux" || os === "macos" || os === "windows" || os === "android" || os === "ios"
}

function buildRimeDictManagerUrl(dir: string): string {
  return `${RIME_DICT_MANAGER_URL_SCHEME}://open?${new URLSearchParams({ dir }).toString()}`
}

interface AppUpdateInfo {
  current_version: string
  latest_version: string
  has_update: boolean
  release_url: string
}

type DownloadSource = "github" | "gitee"

interface PlatformRelease {
  version: string
  download_urls: {
    macos?: string
    windows?: string
    linux?: string
    android?: string
    ios?: string
  }
}

interface ReleaseInfo {
  version: string
  name: string
  published_at: string
  body: string
  github: PlatformRelease | null
  gitee: PlatformRelease | null
}

interface SchemeReleaseInfo {
  scheme: SchemeKey
  sourceType: string | null
  label: string
  version: string
  name: string
  publishedAt: string | null
  downloadUrl: string
  assetName: string
}

interface AppAuthUser {
  id: number
  name: string | null
  nickname: string | null
  email: string | null
}

interface AppAuthSession {
  token: string
  user: AppAuthUser
}

interface UserDictionarySyncResult {
  file_name: string
  path: string
  count: number
  updated_at: string
  import_table_patched: boolean
  reload_stamp_path: string | null
  message: string
}

interface InstallProgress {
  stage: string
  percent: number
  message: string
}

interface FileItem {
  name: string
  is_dir: boolean
}

interface VerifyEntry {
  path: string
  ok: boolean
  note: string
}

interface InstallResult {
  merged_schemas: string[]
  logs: string[]
  verify: VerifyEntry[]
}

interface LocalSchemaInfo {
  installed: boolean
  version: string | null
  schemas: string[]
}

interface DeployResult {
  success: boolean
  message: string
}

interface DeployStep {
  msg: string
  done?: boolean
  error?: boolean
}

interface ComponentVersions {
  app_version: string
  tauri_version: string
  librime_version: string | null
  opencc_version: string | null
  data_dir: string | null
}

interface LinuxImeStatus {
  supported: boolean
  kde_session: boolean
  kde_configured: boolean
  running: boolean
  managed_pid: number | null
  daemon_owner_pid: number | null
  command: string
  processes: string[]
  kde_native_processes: number
  fallback_processes: number
  user_data_dir: string | null
  shared_data_dir: string | null
  shared_data_source: string
  reload_stamp_path: string | null
  reload_stamp_signature: string | null
  message: string
}

interface WindowsImeStatus {
  supported: boolean
  packaged: boolean
  registered: boolean
  registered_dll: boolean
  profile_enabled: boolean
  registration_busy: boolean
  registration_state: string
  registration_error: string | null
  runtime_dir: string | null
  dll_path: string | null
  registered_path: string | null
  profile_status: string
  user_data_dir: string | null
  shared_data_dir: string | null
  shared_data_source: string
  reload_stamp_path: string | null
  reload_stamp_signature: string | null
  message: string
}

interface MacosImeStatus {
  installed: boolean
  app_path: string | null
  user_data_dir: string | null
  shared_data_dir: string | null
  shared_data_source: string
  reload_stamp_path: string | null
  reload_stamp_signature: string | null
  log_dir: string | null
  message: string
}

interface ImeUiSettings {
  colorScheme: ImeUiColorScheme
  effectiveColorScheme: ImeEffectiveColorScheme
  orientation: ImeCandidateOrientation
  accentColor: string
  themePath: string | null
  themeExists: boolean
  reloadStampPath: string | null
  reloadStampSignature: string | null
  message: string
}

interface AndroidImeInputSettings {
  hapticsEnabled: boolean
  hapticIntensity: number
  enterKeyBehavior: EnterKeyBehavior
  configPath: string | null
  reloadStampPath: string | null
  message: string
}

function safUriToDisplayPath(uri: string): string {
  try {
    const treeId = decodeURIComponent(uri.split("/tree/")[1] || "")
    return "/" + treeId.replace("primary:", "sdcard/")
  } catch {
    return uri
  }
}

function logLineClassName(line: string): string {
  if (line.includes("[DEPLOY ERROR]") || line.includes("[ERROR]")) return "text-destructive"
  if (line.includes("[WARN]")) return "text-yellow-400"
  if (line.includes("[DEPLOY]")) return "text-green-400"
  if (line.includes("[MERGED]") || line.includes("[RENAMED]")) return "text-primary"
  return "text-muted-foreground"
}

function GitHubIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      className={className}
      fill="currentColor"
    >
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.09 3.29 9.39 7.86 10.91.58.1.79-.25.79-.55 0-.27-.01-1.17-.02-2.12-3.2.7-3.88-1.36-3.88-1.36-.52-1.33-1.28-1.68-1.28-1.68-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.69 1.25 3.35.96.1-.75.4-1.25.73-1.54-2.56-.29-5.25-1.28-5.25-5.69 0-1.26.45-2.28 1.19-3.08-.12-.29-.52-1.46.11-3.04 0 0 .97-.31 3.17 1.18.92-.26 1.9-.38 2.88-.39.98 0 1.96.13 2.88.39 2.2-1.49 3.17-1.18 3.17-1.18.63 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.42-2.69 5.4-5.26 5.69.41.36.78 1.06.78 2.14 0 1.54-.01 2.78-.01 3.16 0 .31.21.66.79.55A11.51 11.51 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z" />
    </svg>
  )
}

function FileList({
  files,
  loading,
  onRefresh,
  disabled,
}: {
  files: FileItem[]
  loading: boolean
  onRefresh: () => void
  disabled: boolean
}) {
  return (
    <div className="rounded-lg border border-border overflow-hidden">
      <div className="flex items-center justify-between px-3 py-1.5 bg-muted/30 border-b border-border">
        <span className="text-xs text-muted-foreground">{files.length} 个项目</span>
        <button
          onClick={onRefresh}
          disabled={loading || disabled}
          className="text-muted-foreground hover:text-foreground disabled:opacity-40 transition-colors"
          title="刷新"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>
      <div className="max-h-48 overflow-y-auto">
        {loading ? (
          <div className="flex items-center justify-center py-6 text-xs text-muted-foreground gap-2">
            <RefreshCw className="h-3.5 w-3.5 animate-spin" />
            读取中...
          </div>
        ) : files.length === 0 ? (
          <div className="py-6 text-center text-xs text-muted-foreground">目录为空</div>
        ) : (
          files.map((item, i) => (
            <div
              key={i}
              className="flex items-center gap-2 px-3 py-1.5 border-b border-border/40 last:border-0 hover:bg-muted/20"
            >
              {item.is_dir ? (
                <Folder className="h-3.5 w-3.5 text-amber-500 shrink-0" />
              ) : (
                <FileText className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
              )}
              <span className="text-xs font-mono truncate">{item.name}</span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

export default function App() {
  const [osType, setOsType] = useState<OSType>("unknown")
  const [appVersion, setAppVersion] = useState<string>("")
  const [activeTab, setActiveTab] = useState<Tab>("extension")

  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo | null>(null)
  const [releaseError, setReleaseError] = useState<string | null>(null)
  const [isFetchingRelease, setIsFetchingRelease] = useState(true)
  const [downloadSource, setDownloadSource] = useState<DownloadSource>("gitee")
  const [appUpdate, setAppUpdate] = useState<AppUpdateInfo | null>(null)
  const [selectedSchemeKey, setSelectedSchemeKey] = useState<SchemeKey>("keytao")
  const hasManualSchemeSelectionRef = useRef(false)
  const [schemeReleaseInfo, setSchemeReleaseInfo] = useState<SchemeReleaseInfo | null>(null)
  const [schemeReleaseError, setSchemeReleaseError] = useState<string | null>(null)
  const [isFetchingSchemeRelease, setIsFetchingSchemeRelease] = useState(false)
  const [authToken, setAuthToken] = useState<string | null>(null)
  const [authUser, setAuthUser] = useState<AppAuthUser | null>(null)
  const [loginName, setLoginName] = useState("")
  const [loginPassword, setLoginPassword] = useState("")
  const [authError, setAuthError] = useState<string | null>(null)
  const [authMessage, setAuthMessage] = useState<string | null>(null)
  const [isLoggingIn, setIsLoggingIn] = useState(false)
  const [isSyncingUserDictionary, setIsSyncingUserDictionary] = useState(false)

  // Linux IME daemon
  const [linuxImeStatus, setLinuxImeStatus] = useState<LinuxImeStatus | null>(null)
  const [linuxImeError, setLinuxImeError] = useState<string | null>(null)
  const [windowsImeStatus, setWindowsImeStatus] = useState<WindowsImeStatus | null>(null)
  const [windowsImeError, setWindowsImeError] = useState<string | null>(null)
  const [isManagingWindowsIme, setIsManagingWindowsIme] = useState(false)
  const [macosImeStatus, setMacosImeStatus] = useState<MacosImeStatus | null>(null)
  const [macosImeError, setMacosImeError] = useState<string | null>(null)
  const [androidImeStatus, setAndroidImeStatus] = useState<AndroidImeStatus | null>(null)
  const [androidImeError, setAndroidImeError] = useState<string | null>(null)
  const [isCheckingAndroidIme, setIsCheckingAndroidIme] = useState(false)
  const [androidStoragePermission, setAndroidStoragePermission] = useState<AndroidStoragePermissionStatus | null>(null)
  const [androidStoragePermissionError, setAndroidStoragePermissionError] = useState<string | null>(null)
  const [isCheckingAndroidStoragePermission, setIsCheckingAndroidStoragePermission] = useState(false)
  const [imeUiSettings, setImeUiSettings] = useState<ImeUiSettings | null>(null)
  const [imeUiError, setImeUiError] = useState<string | null>(null)
  const [isSavingImeUiSettings, setIsSavingImeUiSettings] = useState(false)
  const [androidImeInputSettings, setAndroidImeInputSettings] = useState<AndroidImeInputSettings | null>(null)
  const [androidImeInputError, setAndroidImeInputError] = useState<string | null>(null)
  const [isSavingAndroidImeInputSettings, setIsSavingAndroidImeInputSettings] = useState(false)
  const [androidHapticIntensityDraft, setAndroidHapticIntensityDraft] = useState<number | null>(null)

  // Default data dir
  const [defaultDir, setDefaultDir] = useState<string | null>(null)

  // Local schema info
  const [localSchemaInfo, setLocalSchemaInfo] = useState<LocalSchemaInfo | null>(null)
  const [isCheckingLocal, setIsCheckingLocal] = useState(false)
  const [isOpeningDir, setIsOpeningDir] = useState(false)
  const [isOpeningDictManager, setIsOpeningDictManager] = useState(false)
  const [isOpeningExtDir, setIsOpeningExtDir] = useState(false)
  const [componentVersions, setComponentVersions] = useState<ComponentVersions | null>(null)

  // Install (default dir)
  const [isInstalling, setIsInstalling] = useState(false)
  const [installProgress, setInstallProgress] = useState<InstallProgress | null>(null)
  const [installError, setInstallError] = useState<string | null>(null)

  // Deploy
  const [isDeploying, setIsDeploying] = useState(false)
  const [deploySteps, setDeploySteps] = useState<DeployStep[]>([])

  // Log buffer
  const [logBuffer, setLogBuffer] = useState<string[]>([])
  const [showLogs, setShowLogs] = useState(false)

  // Changelog
  const [showChangelog, setShowChangelog] = useState(false)

  // Extension tab
  const [selectedDir, setSelectedDir] = useState<string | null>(null)
  const [safUri, setSafUri] = useState<string | null>(null)
  const [files, setFiles] = useState<FileItem[]>([])
  const [isLoadingFiles, setIsLoadingFiles] = useState(false)
  const [localSchemas, setLocalSchemas] = useState<string[] | null>(null)
  const [isInstallingExt, setIsInstallingExt] = useState(false)
  const [extProgress, setExtProgress] = useState<InstallProgress | null>(null)
  const [extError, setExtError] = useState<string | null>(null)
  const [extResult, setExtResult] = useState<InstallResult | null>(null)

  const unlistenInstallRef = useRef<(() => void) | null>(null)
  const unlistenDeployRef = useRef<(() => void) | null>(null)
  const unlistenWindowsImeRef = useRef<(() => void) | null>(null)

  function addLogs(lines: string[]) {
    const ts = new Date().toLocaleTimeString()
    setLogBuffer((prev) => [...prev, ...lines.map((l) => `[${ts}] ${l}`)])
  }

  function persistAuth(token: string, user: AppAuthUser) {
    setAuthToken(token)
    setAuthUser(user)
    try {
      window.localStorage.setItem(AUTH_TOKEN_STORAGE_KEY, token)
      window.localStorage.setItem(AUTH_USER_STORAGE_KEY, JSON.stringify(user))
    } catch {
      // Local storage is a convenience only; the in-memory session remains usable.
    }
  }

  function clearAuth() {
    setAuthToken(null)
    setAuthUser(null)
    setLoginPassword("")
    try {
      window.localStorage.removeItem(AUTH_TOKEN_STORAGE_KEY)
      window.localStorage.removeItem(AUTH_USER_STORAGE_KEY)
    } catch {
      // Ignore storage cleanup failures.
    }
  }

  useEffect(() => {
    try {
      const token = window.localStorage.getItem(AUTH_TOKEN_STORAGE_KEY)
      const rawUser = window.localStorage.getItem(AUTH_USER_STORAGE_KEY)
      if (token) {
        setAuthToken(token)
        if (rawUser) setAuthUser(JSON.parse(rawUser) as AppAuthUser)
      }
    } catch {
      // Ignore invalid persisted auth state.
    }
  }, [])

  useEffect(() => {
    if (!authToken) return
    invoke<AppAuthUser>("keytao_me", { token: authToken })
      .then((user) => {
        setAuthUser(user)
        try {
          window.localStorage.setItem(AUTH_USER_STORAGE_KEY, JSON.stringify(user))
        } catch {
          // Ignore storage update failures.
        }
      })
      .catch((e) => {
        setAuthError(String(e))
        clearAuth()
      })
  }, [authToken])

  useEffect(() => {
    if (selectedSchemeKey === "keytao") {
      setSchemeReleaseInfo(null)
      setSchemeReleaseError(null)
      setIsFetchingSchemeRelease(false)
      return
    }

    setIsFetchingSchemeRelease(true)
    setSchemeReleaseError(null)
    invoke<SchemeReleaseInfo>("fetch_scheme_release", { scheme: selectedSchemeKey })
      .then(setSchemeReleaseInfo)
      .catch((e) => setSchemeReleaseError(String(e)))
      .finally(() => setIsFetchingSchemeRelease(false))
  }, [selectedSchemeKey])

  useEffect(() => {
    if (hasManualSchemeSelectionRef.current) return
    const detectedSchemeKey = schemeKeyFromSchemas(localSchemaInfo?.schemas ?? [])
    if (!detectedSchemeKey) return
    setSelectedSchemeKey((current) => current === detectedSchemeKey ? current : detectedSchemeKey)
  }, [localSchemaInfo?.schemas.join(",")])

  useEffect(() => {
    let windowsImeDisposed = false
    const p = platform()
    const map: Record<string, OSType> = {
      macos: "macos", windows: "windows", linux: "linux",
      android: "android", ios: "ios",
    }
    const os = map[p] ?? "unknown"
    setOsType(os)
    if (hasSystemIme(os)) {
      setActiveTab("ime")
    } else {
      setActiveTab("extension")
    }
    getVersion().then(setAppVersion).catch(() => { })

    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))

    invoke<AppUpdateInfo>("check_app_update")
      .then((info) => { if (info.has_update) setAppUpdate(info) })
      .catch(() => { })

    invoke<string | null>(os === "android" ? "android_keytao_data_dir" : "rime_get_data_dir")
      .then((d) => setDefaultDir(d ?? null))
      .catch(() => { })

    invoke<LocalSchemaInfo>("check_local_schema")
      .then(setLocalSchemaInfo)
      .catch(() => { })

    invoke<ComponentVersions>("get_component_versions")
      .then(setComponentVersions)
      .catch(() => { })

    if (os === "linux") {
      invoke<LinuxImeStatus>("linux_ime_status")
        .then(setLinuxImeStatus)
        .catch((e) => setLinuxImeError(String(e)))
    }
    if (os === "windows") {
      void (async () => {
        const unlisten = await listen<WindowsImeStatus>("windows-ime-status", (e) => {
          setWindowsImeStatus(e.payload)
          setWindowsImeError(e.payload.registration_error ?? null)
          setIsManagingWindowsIme(e.payload.registration_busy)
        })
        if (windowsImeDisposed) {
          unlisten()
          return
        }
        unlistenWindowsImeRef.current = unlisten
        setIsManagingWindowsIme(true)
        setWindowsImeError(null)
        const status = await invoke<WindowsImeStatus>("windows_ime_ensure_registered")
        if (!windowsImeDisposed) {
          setWindowsImeStatus(status)
          setWindowsImeError(status.registration_error ?? null)
          setIsManagingWindowsIme(status.registration_busy)
        }
      })().catch((e) => {
        if (!windowsImeDisposed) {
          setWindowsImeError(String(e))
          setIsManagingWindowsIme(false)
        }
      })
    }
    if (os === "macos") {
      invoke<MacosImeStatus>("macos_ime_status")
        .then(setMacosImeStatus)
        .catch((e) => setMacosImeError(String(e)))
    }
    if (hasSystemIme(os)) {
      invoke<ImeUiSettings>("get_ime_ui_settings")
        .then(setImeUiSettings)
        .catch((e) => setImeUiError(String(e)))
    }
    if (os === "android" || os === "ios") {
      invoke<AndroidImeInputSettings>("get_android_ime_input_settings")
        .then(setAndroidImeInputSettings)
        .catch((e) => setAndroidImeInputError(String(e)))
    }

    listen<InstallProgress>("install-progress", (e) => {
      setInstallProgress(e.payload)
      setExtProgress(e.payload)
    }).then((fn) => { unlistenInstallRef.current = fn })

    return () => {
      windowsImeDisposed = true
      unlistenInstallRef.current?.()
      unlistenDeployRef.current?.()
      unlistenWindowsImeRef.current?.()
    }
  }, [])

  useEffect(() => {
    if (osType !== "android") return
    void refreshAndroidSetupStatus()

    const handleFocus = () => {
      void refreshAndroidSetupStatus()
    }
    const handleVisibilityChange = () => {
      if (document.visibilityState === "visible") {
        void refreshAndroidSetupStatus()
      }
    }

    window.addEventListener("focus", handleFocus)
    document.addEventListener("visibilitychange", handleVisibilityChange)
    return () => {
      window.removeEventListener("focus", handleFocus)
      document.removeEventListener("visibilitychange", handleVisibilityChange)
    }
  }, [osType])

  useEffect(() => {
    setAndroidHapticIntensityDraft(null)
  }, [androidImeInputSettings?.hapticIntensity])

  useEffect(() => {
    if (osType !== "android") return

    const root = document.documentElement
    const isEditableElement = (element: Element | null): element is HTMLElement => {
      return element instanceof HTMLElement && element.matches("input, textarea, [contenteditable='true']")
    }
    const focusedKeyboardFallbackInset = () => {
      return isEditableElement(document.activeElement) ? Math.round(window.innerHeight * 0.42) : 0
    }
    const updateKeyboardInset = () => {
      const viewport = window.visualViewport
      const rawInset = viewport
        ? Math.max(0, window.innerHeight - viewport.height - viewport.offsetTop)
        : 0
      const inset = Math.max(rawInset, focusedKeyboardFallbackInset())
      root.style.setProperty("--android-ime-inset-bottom", `${Math.round(inset)}px`)
      return inset
    }
    const scrollFocusedControlIntoView = () => {
      const active = document.activeElement
      if (!isEditableElement(active)) return
      const align = () => {
        const inset = updateKeyboardInset()
        const rect = active.getBoundingClientRect()
        const topLimit = 24
        const bottomLimit = window.innerHeight - inset - 24
        if (rect.bottom > bottomLimit) {
          window.scrollBy({ top: rect.bottom - bottomLimit, behavior: "smooth" })
        } else if (rect.top < topLimit) {
          window.scrollBy({ top: rect.top - topLimit, behavior: "smooth" })
        }
      }
      align()
      ;[120, 320, 720].forEach((delay) => window.setTimeout(align, delay))
    }
    const handleFocusOut = () => {
      window.setTimeout(updateKeyboardInset, 180)
    }

    updateKeyboardInset()
    window.visualViewport?.addEventListener("resize", updateKeyboardInset)
    window.visualViewport?.addEventListener("scroll", updateKeyboardInset)
    window.addEventListener("resize", updateKeyboardInset)
    document.addEventListener("focusin", scrollFocusedControlIntoView)
    document.addEventListener("focusout", handleFocusOut)
    return () => {
      root.style.removeProperty("--android-ime-inset-bottom")
      window.visualViewport?.removeEventListener("resize", updateKeyboardInset)
      window.visualViewport?.removeEventListener("scroll", updateKeyboardInset)
      window.removeEventListener("resize", updateKeyboardInset)
      document.removeEventListener("focusin", scrollFocusedControlIntoView)
      document.removeEventListener("focusout", handleFocusOut)
    }
  }, [osType])

  const activePlatform = downloadSource === "gitee" ? releaseInfo?.gitee : releaseInfo?.github
  const downloadUrl = activePlatform?.download_urls?.[osType as keyof PlatformRelease["download_urls"]]
  const installedSchemeKey = schemeKeyFromSchemas(localSchemaInfo?.schemas ?? [])
  const installedScheme = installedSchemeKey ? SCHEME_OPTIONS.find((scheme) => scheme.key === installedSchemeKey) : null
  const selectedScheme = SCHEME_OPTIONS.find((scheme) => scheme.key === selectedSchemeKey) ?? SCHEME_OPTIONS[0]
  const selectedSchemeDownloadUrl = selectedSchemeKey === "keytao" ? downloadUrl : schemeReleaseInfo?.downloadUrl
  const selectedSchemeVersion = selectedSchemeKey === "keytao" ? activePlatform?.version : schemeReleaseInfo?.version
  const selectedSchemeAsset = selectedSchemeKey === "keytao" ? selectedScheme.asset : schemeReleaseInfo?.assetName ?? selectedScheme.asset
  const isBusy = isInstalling || isDeploying || isCheckingAndroidStoragePermission
  const systemImeAvailable = hasSystemIme(osType)
  const isMobilePlatform = osType === "android" || osType === "ios"
  const canOpenDefaultDir = osType !== "android"
  const canOpenRimeDictManager = DESKTOP_RIME_DICT_MANAGER_PLATFORMS.includes(osType)
  const imeAccentColor = imeUiSettings?.accentColor ?? DEFAULT_IME_ACCENT_COLOR
  const androidHapticsEnabled = androidImeInputSettings?.hapticsEnabled ?? true
  const androidHapticIntensity = androidHapticIntensityDraft ?? androidImeInputSettings?.hapticIntensity ?? 42
  const enterKeyBehavior = androidImeInputSettings?.enterKeyBehavior ?? "system"
  const androidSetupLoading = isCheckingAndroidIme || isCheckingAndroidStoragePermission || isCheckingLocal
  const androidStorageGranted = androidStoragePermission?.granted ?? false
  const androidSchemaInstalled = localSchemaInfo?.installed ?? false
  const androidImePaddingStyle =
    osType === "android"
      ? { paddingBottom: "calc(1.5rem + var(--android-ime-inset-bottom, 0px))" }
      : undefined
  const shouldShowAndroidImeOnboarding =
    osType === "android" && (
      !androidImeStatus ||
      !androidImeStatus.enabled ||
      !androidImeStatus.selected ||
      !androidStoragePermission ||
      !androidStorageGranted ||
      !androidSchemaInstalled
    )
  const windowsRegistrationBusy = osType === "windows" && (
    isManagingWindowsIme || windowsImeStatus?.registration_busy === true
  )
  const windowsRegistrationState = windowsImeStatus?.registration_state ?? "checking"
  const windowsRegistrationLabel = (() => {
    if (windowsRegistrationBusy) {
      return windowsRegistrationState === "registering" ? "正在注册" : "正在检测"
    }
    if (windowsImeStatus?.registered) return "已注册"
    if (windowsRegistrationState === "failed") return "注册失败"
    if (windowsRegistrationState === "partial") return "部分注册"
    if (windowsRegistrationState === "missing_runtime") return "运行时缺失"
    return windowsImeStatus ? "未注册" : "检测中"
  })()

  async function refreshAndroidImeStatus() {
    setIsCheckingAndroidIme(true)
    try {
      const status = await invoke<AndroidImeStatus>("android_ime_status")
      setAndroidImeStatus(status)
      setAndroidImeError(null)
      return status
    } catch (e) {
      const message = String(e)
      setAndroidImeError(message)
      return null
    } finally {
      setIsCheckingAndroidIme(false)
    }
  }

  async function refreshAndroidStoragePermission() {
    setIsCheckingAndroidStoragePermission(true)
    try {
      const status = await invoke<AndroidStoragePermissionStatus>("android_storage_permission_status")
      setAndroidStoragePermission(status)
      setAndroidStoragePermissionError(null)
      return status
    } catch (e) {
      const message = String(e)
      setAndroidStoragePermissionError(message)
      return null
    } finally {
      setIsCheckingAndroidStoragePermission(false)
    }
  }

  async function refreshAndroidSetupStatus() {
    await Promise.all([
      refreshAndroidImeStatus(),
      refreshAndroidStoragePermission(),
      handleCheckLocalSchema(),
    ])
  }

  async function handleOpenAndroidImeSettings() {
    setAndroidImeError(null)
    try {
      await invoke("android_open_input_method_settings")
    } catch (e) {
      setAndroidImeError(String(e))
    }
  }

  async function handleOpenAndroidStoragePermissionSettings() {
    setAndroidStoragePermissionError(null)
    try {
      await invoke("android_open_storage_permission_settings")
      window.setTimeout(() => {
        void refreshAndroidStoragePermission()
      }, 800)
    } catch (e) {
      setAndroidStoragePermissionError(String(e))
    }
  }

  async function handleShowAndroidImePicker() {
    setAndroidImeError(null)
    try {
      await invoke("android_show_input_method_picker")
      window.setTimeout(() => {
        void refreshAndroidImeStatus()
      }, 800)
    } catch (e) {
      setAndroidImeError(String(e))
    }
  }

  async function handleCheckLocalSchema() {
    setIsCheckingLocal(true)
    try {
      const info = await invoke<LocalSchemaInfo>("check_local_schema")
      setLocalSchemaInfo(info)
    } catch { }
    finally { setIsCheckingLocal(false) }
  }

  async function handleOpenDefaultDir() {
    if (!defaultDir) return
    setIsOpeningDir(true)
    try {
      await openPath(defaultDir)
    } catch (e) {
      addLogs([`[OPEN DIR ERROR] ${String(e)}`])
    } finally {
      setIsOpeningDir(false)
    }
  }

  async function handleOpenRimeDictManager() {
    if (!defaultDir || !canOpenRimeDictManager) return
    setIsOpeningDictManager(true)
    try {
      await openUrl(buildRimeDictManagerUrl(defaultDir))
    } catch (e) {
      addLogs([`[OPEN RIME DICT ERROR] ${String(e)}`])
    } finally {
      setIsOpeningDictManager(false)
    }
  }

  async function handleOpenSelectedDir() {
    if (!selectedDir) return
    setIsOpeningExtDir(true)
    try {
      await openPath(selectedDir)
    } catch (e) {
      addLogs([`[OPEN DIR ERROR] ${String(e)}`])
    } finally {
      setIsOpeningExtDir(false)
    }
  }

  async function handleOpenImeTheme() {
    if (!imeUiSettings?.themePath) return
    try {
      await openPath(imeUiSettings.themePath)
    } catch (e) {
      addLogs([`[OPEN THEME ERROR] ${String(e)}`])
    }
  }

  async function handleOpenGitHubRepository() {
    try {
      await openUrl(GITHUB_REPOSITORY_URL)
    } catch (e) {
      addLogs([`[OPEN URL ERROR] ${String(e)}`])
    }
  }

  async function handleDeploy() {
    setIsDeploying(true)
    const steps: DeployStep[] = [{ msg: "正在部署 librime..." }]
    setDeploySteps([...steps])

    unlistenDeployRef.current?.()
    const unlisten = await listen<string>("deploy-progress", (e) => {
      steps.push({ msg: e.payload })
      setDeploySteps([...steps])
    })
    unlistenDeployRef.current = unlisten

    try {
      const result = await invoke<DeployResult>("rime_deploy_default")
      steps.push({ msg: result.message, done: true })
      setDeploySteps([...steps])
      addLogs([`[DEPLOY] ${result.message}`])
    } catch (e) {
      const msg = String(e)
      steps.push({ msg, error: true })
      setDeploySteps([...steps])
      addLogs([`[DEPLOY ERROR] ${msg}`])
    } finally {
      setIsDeploying(false)
    }
  }

  async function handleWindowsImeRefresh() {
    if (isManagingWindowsIme) return
    setIsManagingWindowsIme(true)
    setWindowsImeError(null)
    try {
      const status = await invoke<WindowsImeStatus>("windows_ime_status")
      setWindowsImeStatus(status)
      setWindowsImeError(status.registration_error ?? null)
      addLogs([`[WINDOWS IME] ${status.message}`])
    } catch (e) {
      const message = String(e)
      setWindowsImeError(message)
      addLogs([`[WINDOWS IME ERROR] ${message}`])
    } finally {
      setIsManagingWindowsIme(false)
    }
  }

  async function handleUpdateImeUiSettings(
    patch: Partial<Pick<ImeUiSettings, "colorScheme" | "orientation" | "accentColor">>,
  ) {
    if (isSavingImeUiSettings) return
    const current = {
      colorScheme: imeUiSettings?.colorScheme ?? "auto",
      orientation: imeUiSettings?.orientation ?? "horizontal",
      accentColor: imeUiSettings?.accentColor ?? DEFAULT_IME_ACCENT_COLOR,
    }
    const next = { ...current, ...patch }
    if (
      imeUiSettings &&
      next.colorScheme === imeUiSettings.colorScheme &&
      next.orientation === imeUiSettings.orientation &&
      next.accentColor === imeUiSettings.accentColor
    ) {
      return
    }
    setIsSavingImeUiSettings(true)
    setImeUiError(null)
    try {
      const settings = await invoke<ImeUiSettings>("set_ime_ui_settings", next)
      setImeUiSettings(settings)
      addLogs([`[IME UI] ${settings.message}`])
    } catch (e) {
      setImeUiError(String(e))
    } finally {
      setIsSavingImeUiSettings(false)
    }
  }

  async function refreshAndroidImeInputSettings() {
    try {
      const settings = await invoke<AndroidImeInputSettings>("get_android_ime_input_settings")
      setAndroidImeInputSettings(settings)
      setAndroidImeInputError(null)
      return settings
    } catch (e) {
      setAndroidImeInputError(String(e))
      return null
    }
  }

  async function handleUpdateAndroidImeInputSettings(
    patch: Partial<Pick<AndroidImeInputSettings, "hapticsEnabled" | "hapticIntensity" | "enterKeyBehavior">>,
  ) {
    if (!["android", "ios"].includes(osType) || isSavingAndroidImeInputSettings) return
    const current = {
      hapticsEnabled: androidImeInputSettings?.hapticsEnabled ?? true,
      hapticIntensity: androidImeInputSettings?.hapticIntensity ?? 42,
      enterKeyBehavior: androidImeInputSettings?.enterKeyBehavior ?? ("system" as EnterKeyBehavior),
    }
    const next = {
      ...current,
      ...patch,
      hapticIntensity: Math.round(patch.hapticIntensity ?? current.hapticIntensity),
    }
    next.enterKeyBehavior = next.enterKeyBehavior === "newline" ? "newline" : "system"
    next.hapticIntensity = Math.min(100, Math.max(1, next.hapticIntensity))
    if (
      androidImeInputSettings &&
      next.hapticsEnabled === androidImeInputSettings.hapticsEnabled &&
      next.hapticIntensity === androidImeInputSettings.hapticIntensity &&
      next.enterKeyBehavior === androidImeInputSettings.enterKeyBehavior
    ) {
      return
    }
    setIsSavingAndroidImeInputSettings(true)
    setAndroidImeInputError(null)
    try {
      const settings = await invoke<AndroidImeInputSettings>("set_android_ime_input_settings", next)
      setAndroidImeInputSettings(settings)
      addLogs([`[MOBILE IME INPUT] ${settings.message}`])
    } catch (e) {
      setAndroidImeInputError(String(e))
      await refreshAndroidImeInputSettings()
    } finally {
      setIsSavingAndroidImeInputSettings(false)
    }
  }

  function commitAndroidHapticIntensity(value: number) {
    void handleUpdateAndroidImeInputSettings({ hapticIntensity: value })
  }

  async function handleInstall() {
    if (!selectedSchemeDownloadUrl) return
    setInstallProgress(null)
    setInstallError(null)
    setDeploySteps([])

    if (osType === "android") {
      const permission = await refreshAndroidStoragePermission()
      if (!permission?.granted) {
        const message = permission?.message || ANDROID_STORAGE_PERMISSION_MESSAGE
        setInstallError(message)
        addLogs([`[ANDROID PERMISSION] ${message}`])
        await handleOpenAndroidStoragePermissionSettings()
        return
      }
    }

    setIsInstalling(true)
    try {
      const result = await invoke<InstallResult>("rime_install_to_default", { url: selectedSchemeDownloadUrl })
      addLogs(result.logs)
      if (result.verify.some((v) => !v.ok)) {
        addLogs(result.verify.filter((v) => !v.ok).map((v) => `[VERIFY FAIL] ${v.path}: ${v.note}`))
      }
      await handleCheckLocalSchema()
    } catch (e) {
      setInstallError(String(e))
      setIsInstalling(false)
      return
    }
    setIsInstalling(false)
    await handleDeploy()
  }

  async function handleRefetchRelease() {
    if (selectedSchemeKey !== "keytao") {
      setIsFetchingSchemeRelease(true)
      setSchemeReleaseError(null)
      invoke<SchemeReleaseInfo>("fetch_scheme_release", { scheme: selectedSchemeKey })
        .then(setSchemeReleaseInfo)
        .catch((e) => setSchemeReleaseError(String(e)))
        .finally(() => setIsFetchingSchemeRelease(false))
      return
    }

    setIsFetchingRelease(true)
    setReleaseError(null)
    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))
  }

  async function handleLogin() {
    if (isLoggingIn) return
    setIsLoggingIn(true)
    setAuthError(null)
    setAuthMessage(null)
    try {
      const session = await invoke<AppAuthSession>("keytao_login", {
        name: loginName,
        password: loginPassword,
      })
      persistAuth(session.token, session.user)
      setLoginPassword("")
      setAuthMessage("已登录 KeyTao 账号")
    } catch (e) {
      setAuthError(String(e))
    } finally {
      setIsLoggingIn(false)
    }
  }

  function handleLogout() {
    clearAuth()
    setAuthError(null)
    setAuthMessage("已退出登录")
  }

  async function handleSyncUserDictionary() {
    if (!authToken || isSyncingUserDictionary) return
    setIsSyncingUserDictionary(true)
    setAuthError(null)
    setAuthMessage("正在同步用户词库...")
    try {
      const result = await invoke<UserDictionarySyncResult>("sync_user_dictionary", { token: authToken })
      setAuthMessage(`${result.message}，已写入 ${result.file_name}`)
      addLogs([
        `[USER DICT] ${result.message}`,
        `[USER DICT] ${result.path}`,
        result.import_table_patched ? "[USER DICT] 已更新词典导入表" : "[USER DICT] 词典导入表已就绪",
      ])
      await handleDeploy()
      await handleCheckLocalSchema()
    } catch (e) {
      setAuthError(String(e))
    } finally {
      setIsSyncingUserDictionary(false)
    }
  }

  async function loadFiles(path?: string, uri?: string) {
    setIsLoadingFiles(true)
    try {
      if (osType === "android" && uri) {
        const [items, info] = await Promise.all([
          invoke<FileItem[]>("android_list_files", { treeUri: uri }),
          invoke<LocalSchemaInfo>("android_read_local_schemas", { treeUri: uri }).catch(() => null),
        ])
        setFiles(items)
        if (info) {
          setLocalSchemas(info.schemas)
          setLocalSchemaInfo(info)
        } else {
          setLocalSchemas(null)
        }
      } else if (path) {
        const [items, schemas] = await Promise.all([
          invoke<FileItem[]>("list_dir", { path }),
          invoke<string[]>("read_local_schemas", { path }).catch(() => null),
        ])
        setFiles(items)
        setLocalSchemas(schemas)
      }
    } catch {
      setFiles([])
      setLocalSchemas(null)
    } finally {
      setIsLoadingFiles(false)
    }
  }

  async function handleSelectDir() {
    setLocalSchemas(null)
    setFiles([])
    if (osType === "android") {
      try {
        const { uri } = await invoke<{ uri: string }>("android_pick_directory")
        setSafUri(uri)
        setSelectedDir(safUriToDisplayPath(uri))
        setExtResult(null)
        setExtError(null)
        await loadFiles(undefined, uri)
      } catch (e) {
        setExtError(String(e))
      }
    } else {
      try {
        const dir = await invoke<string | null>("select_directory", { imType: null })
        if (dir) {
          setSelectedDir(dir)
          setSafUri(null)
          setExtResult(null)
          setExtError(null)
          await loadFiles(dir)
        }
      } catch (e) {
        setExtError(String(e))
      }
    }
  }

  async function handleInstallExt() {
    if (!selectedDir || !selectedSchemeDownloadUrl) return
    setIsInstallingExt(true)
    setExtResult(null)
    setExtError(null)
    setExtProgress(null)
    try {
      const tempPath = await invoke<string>("download_to_temp", { url: selectedSchemeDownloadUrl })
      let result: InstallResult
      if (osType === "android" && safUri) {
        result = await invoke<InstallResult>("android_smart_extract", { zipPath: tempPath, treeUri: safUri })
      } else {
        result = await invoke<InstallResult>("smart_install", { zipPath: tempPath, destPath: selectedDir })
      }
      setExtResult(result)
      addLogs(result.logs)
      await loadFiles(selectedDir ?? undefined, safUri ?? undefined)
    } catch (e) {
      setExtError(String(e))
    } finally {
      setIsInstallingExt(false)
      setExtProgress(null)
    }
  }

  // ── Release source picker (shared widget) ────────────────────────────────
  const VersionPicker = (
    <div className="flex items-center gap-1.5">
      {selectedSchemeKey === "keytao" && releaseInfo?.github && (
        <div className="flex gap-1">
          {(["github", "gitee"] as const).map((src) => {
            const p = src === "github" ? releaseInfo.github : releaseInfo.gitee
            if (!p) return null
            return (
              <button
                key={src}
                onClick={() => setDownloadSource(src)}
                className={`px-2 py-0.5 text-xs rounded border transition-colors font-mono ${downloadSource === src
                  ? "bg-primary text-primary-foreground border-primary"
                  : "bg-transparent text-muted-foreground border-border hover:border-foreground/40"
                  }`}
              >
                {src === "github" ? "GitHub" : "Gitee"} {p.version}
              </button>
            )
          })}
        </div>
      )}
      {selectedSchemeKey === "keytao" && releaseInfo && !releaseInfo.github && (
        <Badge variant="secondary" className="font-mono text-xs">{releaseInfo.version}</Badge>
      )}
      {selectedSchemeKey !== "keytao" && selectedSchemeVersion && (
        <Badge variant="secondary" className="font-mono text-xs">
          {selectedSchemeVersion} · {selectedSchemeAsset}
        </Badge>
      )}
      {selectedSchemeKey === "keytao" && releaseInfo?.body && (
        <button
          onClick={() => setShowChangelog(true)}
          className="text-xs text-muted-foreground hover:text-foreground transition-colors underline underline-offset-2"
        >
          更新内容
        </button>
      )}
      {isFetchingRelease || isFetchingSchemeRelease
        ? <RefreshCw className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
        : <Button variant="ghost" size="icon" className="h-6 w-6" onClick={handleRefetchRelease} title="检查新版本">
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
      }
    </div>
  )

  if (shouldShowAndroidImeOnboarding) {
    return (
      <AndroidImeOnboarding
        status={androidImeStatus}
        storageStatus={androidStoragePermission}
        schemaInstalled={androidSchemaInstalled}
        loading={androidSetupLoading}
        error={androidImeError}
        storageError={androidStoragePermissionError}
        installError={installError}
        installingSchema={isInstalling || isDeploying}
        canInstallSchema={Boolean(selectedSchemeDownloadUrl)}
        onOpenSettings={handleOpenAndroidImeSettings}
        onShowPicker={handleShowAndroidImePicker}
        onOpenStorageSettings={handleOpenAndroidStoragePermissionSettings}
        onInstallSchema={handleInstall}
        onRefresh={refreshAndroidSetupStatus}
      />
    )
  }

  return (
    <div className="h-screen overflow-y-auto bg-background text-foreground">
      <div className="max-w-2xl mx-auto px-4 pt-6 pb-[calc(2rem+env(safe-area-inset-bottom))] space-y-4" style={androidImePaddingStyle}>

        {/* Header */}
        <div className="flex items-center gap-3 pb-1">
          <img src="/logo.png" alt="KeyTao" className="h-12 w-12" />
          <div>
            <h1 className="text-xl font-bold tracking-tight leading-tight">
              KeyTao 键道
              {appVersion && <span className="ml-2 text-sm font-normal text-muted-foreground">v{appVersion}</span>}
            </h1>
            <p className="text-xs text-muted-foreground">键道，基于 librime 的跨平台原生输入法</p>
          </div>
        </div>

        {/* App update banner */}
        {appUpdate && (
          <a
            href={appUpdate.release_url}
            target="_blank"
            rel="noreferrer"
            className="flex items-center justify-between gap-3 px-4 py-2.5 rounded-lg border border-primary/30 bg-primary/5 text-sm hover:bg-primary/10 transition-colors"
          >
            <div className="flex items-center gap-2">
              <Download className="h-4 w-4 text-primary shrink-0" />
              <span>KeyTao 有新版本可用</span>
              <Badge variant="secondary" className="font-mono text-xs">v{appUpdate.latest_version}</Badge>
            </div>
            <div className="flex items-center gap-2 text-muted-foreground text-xs shrink-0">
              <span>当前 v{appUpdate.current_version}</span>
              <ExternalLink className="h-3 w-3" />
            </div>
          </a>
        )}

        {/* Tab nav */}
        <div className="flex border-b border-border">
          {([
            ...(systemImeAvailable ? [{ id: "ime" as const, label: "输入法", icon: Keyboard }] : []),
            { id: "extension", label: "扩展安装", icon: Settings },
            { id: "about", label: "关于", icon: Info },
            { id: "debug", label: "调试", icon: ScrollText },
          ] as { id: Tab; label: string; icon: LucideIcon }[]).map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setActiveTab(id)}
              className={`flex items-center gap-1 px-2.5 py-2.5 text-xs font-medium border-b-2 transition-colors -mb-px whitespace-nowrap ${activeTab === id
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground hover:border-border"
                }`}
            >
              <Icon className="h-3.5 w-3.5 shrink-0" />
              {label}
            </button>
          ))}
        </div>

        {/* ══ 输入法 Tab ════════════════════════════════════════════════════ */}
        {activeTab === "ime" && systemImeAvailable && (
          <div className="space-y-4">
            {osType === "windows" && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Keyboard className="h-4 w-4 text-muted-foreground" />
                    Windows 系统输入法
                    <span className="ml-auto">
                      {windowsRegistrationBusy ? (
                        <Badge variant="outline" className="text-xs gap-1">
                          <Loader2 className="h-3 w-3 animate-spin" />
                          {windowsRegistrationLabel}
                        </Badge>
                      ) : windowsImeStatus?.registered ? (
                        <Badge className="text-xs gap-1 bg-green-500/20 text-green-400 border-green-500/30"><CheckCircle2 className="h-3 w-3" />已注册</Badge>
                      ) : windowsRegistrationState === "failed" ? (
                        <Badge variant="outline" className="text-xs gap-1 border-destructive/40 text-destructive">
                          <AlertTriangle className="h-3 w-3" />
                          注册失败
                        </Badge>
                      ) : (
                        <Badge variant="outline" className="text-xs">{windowsRegistrationLabel}</Badge>
                      )}
                    </span>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {!windowsImeStatus && (
                    <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
                      <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" />
                      正在检测 KeyTao Windows IME 状态
                    </div>
                  )}
                  {windowsImeStatus && (
                    <div className="grid gap-2 text-xs">
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge variant={windowsImeStatus.packaged ? "default" : "outline"} className="text-xs">
                          安装包 {windowsImeStatus.packaged ? "完整" : "缺少 IME 运行时"}
                        </Badge>
                        <Badge
                          variant={windowsImeStatus.registered ? "default" : "outline"}
                          className={cn(
                            "text-xs",
                            windowsRegistrationState === "failed" && "border-destructive/40 text-destructive",
                          )}
                        >
                          注册 {windowsRegistrationLabel}
                        </Badge>
                        <Badge variant={windowsImeStatus.registered_dll ? "default" : "outline"} className="text-xs">
                          DLL {windowsImeStatus.registered_dll ? "匹配" : "未匹配"}
                        </Badge>
                        <Badge variant={windowsImeStatus.profile_enabled ? "default" : "outline"} className="text-xs">
                          Profile {windowsImeStatus.profile_enabled ? "已启用" : "不可用"}
                        </Badge>
                      </div>
                      {windowsImeStatus.runtime_dir && (
                        <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2">
                          <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="text-muted-foreground">运行时：</span>
                          <code className="font-mono truncate">{windowsImeStatus.runtime_dir}</code>
                        </div>
                      )}
                      {windowsImeStatus.registered_path && (
                        <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2">
                          <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="text-muted-foreground">已注册：</span>
                          <code className="font-mono truncate">{windowsImeStatus.registered_path}</code>
                        </div>
                      )}
                      {windowsImeStatus.profile_status && (
                        <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2">
                          <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="text-muted-foreground">Profile：</span>
                          <code className="font-mono truncate">{windowsImeStatus.profile_status}</code>
                        </div>
                      )}
                      {windowsImeStatus.message && (
                        <div className="text-xs text-muted-foreground rounded-lg border border-border bg-muted/20 px-3 py-2">
                          {windowsImeStatus.message}
                        </div>
                      )}
                    </div>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleWindowsImeRefresh}
                      disabled={windowsRegistrationBusy}
                      className="gap-1.5"
                    >
                      <RefreshCw className={`h-3.5 w-3.5 ${windowsRegistrationBusy ? "animate-spin" : ""}`} />
                      刷新
                    </Button>
                  </div>
                  {windowsImeError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{windowsImeError}</span>
                    </div>
                  )}
                </CardContent>
              </Card>
            )}

            {osType === "macos" && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Keyboard className="h-4 w-4 text-muted-foreground" />
                    macOS 系统输入法
                    <span className="ml-auto">
                      {macosImeStatus?.installed
                        ? <Badge className="text-xs gap-1 bg-green-500/20 text-green-400 border-green-500/30"><CheckCircle2 className="h-3 w-3" />已安装</Badge>
                        : <Badge variant="outline" className="text-xs">未安装</Badge>
                      }
                    </span>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {macosImeStatus?.app_path && (
                    <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs">
                      <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      <span className="text-muted-foreground">系统位置：</span>
                      <code className="font-mono truncate">{macosImeStatus.app_path}</code>
                    </div>
                  )}
                  {macosImeError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{macosImeError}</span>
                    </div>
                  )}
                </CardContent>
              </Card>
            )}

            {osType === "linux" && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Cpu className="h-4 w-4 text-muted-foreground" />
                    Linux 系统输入法
                    <span className="ml-auto">
                      {linuxImeStatus?.running
                        ? <Badge className="text-xs gap-1 bg-green-500/20 text-green-400 border-green-500/30"><CheckCircle2 className="h-3 w-3" />运行中</Badge>
                        : <Badge variant="outline" className="text-xs">未启动</Badge>
                      }
                    </span>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {linuxImeStatus && (
                    <div className="grid gap-2 text-xs">
                      <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2">
                        <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                        <span className="text-muted-foreground">命令：</span>
                        <code className="font-mono truncate">{linuxImeStatus.command}</code>
                      </div>
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge variant="secondary" className="text-xs">
                          {linuxImeStatus.kde_session ? "KDE 会话" : "非 KDE 会话"}
                        </Badge>
                        <Badge variant={linuxImeStatus.kde_configured ? "default" : "outline"} className="text-xs">
                          KDE {linuxImeStatus.kde_configured ? "已配置" : "未配置"}
                        </Badge>
                        {linuxImeStatus.managed_pid && (
                          <Badge variant="outline" className="text-xs font-mono">pid {linuxImeStatus.managed_pid}</Badge>
                        )}
                        {linuxImeStatus.kde_native_processes > 0 && (
                          <Badge variant="outline" className="text-xs">KWIN_WAYLAND {linuxImeStatus.kde_native_processes}</Badge>
                        )}
                        {linuxImeStatus.fallback_processes > 0 && (
                          <Badge variant="outline" className="text-xs">XIM+IBUS {linuxImeStatus.fallback_processes}</Badge>
                        )}
                      </div>
                      {linuxImeStatus.message && (
                        <div className="text-xs text-muted-foreground rounded-lg border border-border bg-muted/20 px-3 py-2">
                          {linuxImeStatus.message}
                        </div>
                      )}
                    </div>
                  )}
                  {linuxImeError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{linuxImeError}</span>
                    </div>
                  )}
                </CardContent>
              </Card>
            )}

            {systemImeAvailable && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Palette className="h-4 w-4 text-muted-foreground" />
                    输入法外观
                    {isSavingImeUiSettings && (
                      <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />
                    )}
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div className="grid grid-cols-3 gap-1 rounded-lg border border-border bg-muted/30 p-1">
                    {([
                      { id: "auto" as const, label: "自动", icon: Monitor },
                      { id: "light" as const, label: "白天", icon: Sun },
                      { id: "dark" as const, label: "夜间", icon: Moon },
                    ]).map(({ id, label, icon: Icon }) => {
                      const selected = (imeUiSettings?.colorScheme ?? "auto") === id
                      return (
                        <Button
                          key={id}
                          type="button"
                          variant={selected ? "secondary" : "ghost"}
                          size="sm"
                          onClick={() => handleUpdateImeUiSettings({ colorScheme: id })}
                          disabled={isSavingImeUiSettings}
                          className={cn(
                            "h-8 rounded-md text-xs",
                            !selected && "text-muted-foreground hover:text-foreground",
                          )}
                        >
                          <Icon className="h-3.5 w-3.5" />
                          {label}
                        </Button>
                      )
                    })}
                  </div>
                  {!isMobilePlatform && (
                    <div className="grid grid-cols-2 gap-1 rounded-lg border border-border bg-muted/30 p-1">
                      {([
                        { id: "horizontal" as const, label: "横排", icon: Columns3 },
                        { id: "vertical" as const, label: "竖排", icon: Rows3 },
                      ]).map(({ id, label, icon: Icon }) => {
                        const selected = (imeUiSettings?.orientation ?? "horizontal") === id
                        return (
                          <Button
                            key={id}
                            type="button"
                            variant={selected ? "secondary" : "ghost"}
                            size="sm"
                            onClick={() => handleUpdateImeUiSettings({ orientation: id })}
                            disabled={isSavingImeUiSettings}
                            className={cn(
                              "h-8 rounded-md text-xs",
                              !selected && "text-muted-foreground hover:text-foreground",
                            )}
                          >
                            <Icon className="h-3.5 w-3.5" />
                            {label}
                          </Button>
                        )
                      })}
                    </div>
                  )}
                  <div className="grid grid-cols-1 gap-3 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs sm:grid-cols-[minmax(8.5rem,1fr)_auto] sm:items-center">
                    <div className="flex min-w-0 items-center gap-2 whitespace-nowrap">
                      <Paintbrush className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      <span className="shrink-0 text-muted-foreground">主题色</span>
                      {imeUiSettings && (
                        <Badge variant="outline" className="shrink-0 text-xs">
                          {imeUiSettings.colorScheme === "auto"
                            ? "跟随系统"
                            : imeUiSettings.colorScheme === "dark"
                              ? "固定夜间"
                              : "固定白天"}
                        </Badge>
                      )}
                    </div>
                    <ColorPicker
                      value={imeAccentColor}
                      onChange={(accentColor) => handleUpdateImeUiSettings({ accentColor })}
                      disabled={isSavingImeUiSettings}
                      presets={CROSS_PLATFORM_IME_ACCENT_PRESETS}
                      className="justify-start sm:justify-end"
                      title="选择主题色"
                    />
                  </div>
                  {imeUiSettings?.themePath && (
                    <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs">
                      <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      <span className="text-muted-foreground">主题：</span>
                      <code className="font-mono truncate flex-1 min-w-0">{imeUiSettings.themePath}</code>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={handleOpenImeTheme}
                        disabled={!imeUiSettings.themeExists}
                        className="h-6 w-6 shrink-0"
                        title="打开主题配置"
                      >
                        <FolderOpen className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  )}
                  {imeUiError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{imeUiError}</span>
                    </div>
                  )}
                </CardContent>
              </Card>
            )}

            {(osType === "android" || osType === "ios") && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    {androidHapticsEnabled ? (
                      <Vibrate className="h-4 w-4 text-muted-foreground" />
                    ) : (
                      <VibrateOff className="h-4 w-4 text-muted-foreground" />
                    )}
                    移动端输入反馈
                    {isSavingAndroidImeInputSettings && (
                      <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />
                    )}
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div className="flex items-center justify-between gap-3 rounded-lg border border-border bg-muted/40 px-3 py-2">
                    <div className="flex min-w-0 items-center gap-2 text-xs">
                      {androidHapticsEnabled ? (
                        <Vibrate className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      ) : (
                        <VibrateOff className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      )}
                      <span className="text-muted-foreground">键盘震动</span>
                      <span className="text-xs text-muted-foreground/80">
                        {androidHapticsEnabled ? "开启" : "关闭"}
                      </span>
                    </div>
                    <Switch
                      checked={androidHapticsEnabled}
                      onCheckedChange={(checked) => handleUpdateAndroidImeInputSettings({ hapticsEnabled: checked })}
                      disabled={isSavingAndroidImeInputSettings}
                      aria-label="切换键盘震动"
                    />
                  </div>
                  <div className="rounded-lg border border-border bg-muted/40 px-3 py-2.5 space-y-2">
                    <div className="flex items-center justify-between gap-3 text-xs">
                      <div className="flex min-w-0 items-center gap-2">
                        <SlidersHorizontal className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                        <span className="text-muted-foreground">震动强度</span>
                      </div>
                      <span className="font-mono text-muted-foreground">{androidHapticIntensity}%</span>
                    </div>
                    <Slider
                      min={1}
                      max={100}
                      step={1}
                      value={[androidHapticIntensity]}
                      onValueChange={([value]) => setAndroidHapticIntensityDraft(value)}
                      onValueCommit={([value]) => commitAndroidHapticIntensity(value)}
                      disabled={!androidHapticsEnabled || isSavingAndroidImeInputSettings}
                      aria-label="震动强度"
                    />
                  </div>
                  <div className="rounded-lg border border-border bg-muted/40 px-3 py-2.5 space-y-2">
                    <div className="flex items-center justify-between gap-3 text-xs">
                      <div className="flex min-w-0 items-center gap-2">
                        <Keyboard className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                        <span className="text-muted-foreground">回车键</span>
                      </div>
                      <span className="text-xs text-muted-foreground/80">
                        {enterKeyBehavior === "newline" ? "输入换行" : "跟随系统"}
                      </span>
                    </div>
                    <div className="grid grid-cols-2 gap-2">
                      <Button
                        type="button"
                        variant={enterKeyBehavior === "system" ? "default" : "outline"}
                        size="sm"
                        onClick={() => handleUpdateAndroidImeInputSettings({ enterKeyBehavior: "system" })}
                        disabled={isSavingAndroidImeInputSettings}
                        className="h-8 text-xs"
                      >
                        跟随系统
                      </Button>
                      <Button
                        type="button"
                        variant={enterKeyBehavior === "newline" ? "default" : "outline"}
                        size="sm"
                        onClick={() => handleUpdateAndroidImeInputSettings({ enterKeyBehavior: "newline" })}
                        disabled={isSavingAndroidImeInputSettings}
                        className="h-8 text-xs"
                      >
                        换行
                      </Button>
                    </div>
                  </div>
                  {androidImeInputSettings?.configPath && (
                    <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs">
                      <Info className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      <span className="text-muted-foreground">配置：</span>
                      <code className="font-mono truncate flex-1 min-w-0">{androidImeInputSettings.configPath}</code>
                    </div>
                  )}
                  {androidImeInputError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{androidImeInputError}</span>
                    </div>
                  )}
                </CardContent>
              </Card>
            )}



            {systemImeAvailable && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <User className="h-4 w-4 text-muted-foreground" />
                    KeyTao 账号
                    {authUser && (
                      <Badge variant="secondary" className="ml-auto text-xs">
                        {authUser.nickname || authUser.name || `#${authUser.id}`}
                      </Badge>
                    )}
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {authUser ? (
                    <div className="flex flex-wrap items-center gap-2">
                      <Button
                        size="sm"
                        onClick={handleSyncUserDictionary}
                        disabled={isSyncingUserDictionary || isDeploying}
                        className="gap-1.5"
                      >
                        {isSyncingUserDictionary || isDeploying
                          ? <Loader2 className="h-4 w-4 animate-spin" />
                          : <Cloud className="h-4 w-4" />
                        }
                        {isSyncingUserDictionary ? "同步中..." : isDeploying ? "部署中..." : "同步用户词库"}
                      </Button>
                      <Button variant="outline" size="sm" onClick={handleLogout} className="gap-1.5">
                        <LogOut className="h-3.5 w-3.5" />
                        退出
                      </Button>
                    </div>
                  ) : (
                    <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
                      <input
                        value={loginName}
                        onChange={(event) => setLoginName(event.target.value)}
                        placeholder="用户名"
                        className="h-9 min-w-0 rounded-md border border-border bg-background px-3 text-sm outline-none focus:ring-1 focus:ring-primary"
                      />
                      <input
                        value={loginPassword}
                        onChange={(event) => setLoginPassword(event.target.value)}
                        placeholder="密码"
                        type="password"
                        className="h-9 min-w-0 rounded-md border border-border bg-background px-3 text-sm outline-none focus:ring-1 focus:ring-primary"
                        onKeyDown={(event) => {
                          if (event.key === "Enter") void handleLogin()
                        }}
                      />
                      <Button size="sm" onClick={handleLogin} disabled={isLoggingIn} className="gap-1.5">
                        {isLoggingIn ? <Loader2 className="h-4 w-4 animate-spin" /> : <LogIn className="h-4 w-4" />}
                        登录
                      </Button>
                    </div>
                  )}
                  {authMessage && (
                    <div className="text-xs text-green-400 rounded-lg border border-green-500/30 bg-green-500/10 px-3 py-2">
                      {authMessage}
                    </div>
                  )}
                  {authError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{authError}</span>
                    </div>
                  )}
                </CardContent>
              </Card>
            )}

            {systemImeAvailable && (
              <Card>
                <CardHeader className="pb-3 space-y-2">
                  <CardTitle className="text-sm font-semibold flex items-center gap-2">
                    <Download className="h-4 w-4 text-muted-foreground" />
                    键道方案
                  </CardTitle>
                  <div className="flex flex-wrap items-center gap-1.5">{VersionPicker}</div>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                    {SCHEME_OPTIONS.map((scheme) => {
                      const selected = selectedSchemeKey === scheme.key
                      return (
                        <Button
                          key={scheme.key}
                          type="button"
                          variant={selected ? "secondary" : "outline"}
                          size="sm"
                          onClick={() => {
                            hasManualSchemeSelectionRef.current = true
                            setSelectedSchemeKey(scheme.key)
                          }}
                          disabled={isInstalling || isDeploying || isFetchingSchemeRelease}
                          className="h-10 min-w-0 flex-col gap-0 px-2"
                          title={scheme.asset}
                        >
                          <span className="text-xs leading-tight">{scheme.label}</span>
                          <span className="max-w-full truncate font-mono text-[10px] leading-tight text-muted-foreground">
                            {scheme.key === selectedSchemeKey && selectedSchemeVersion ? selectedSchemeVersion : scheme.asset}
                          </span>
                        </Button>
                      )
                    })}
                  </div>
                  <div className="flex items-center gap-2 text-xs text-muted-foreground bg-muted/30 border border-border rounded-lg px-3 py-2">
                    <Download className="h-3.5 w-3.5 shrink-0" />
                    <span className="shrink-0">选择：</span>
                    <span className="min-w-0 truncate">
                      {selectedScheme.label}
                      {selectedSchemeVersion ? ` ${selectedSchemeVersion}` : ""}
                      {selectedSchemeAsset ? ` · ${selectedSchemeAsset}` : ""}
                    </span>
                  </div>
                  {defaultDir && (
                    <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground bg-muted/40 border border-border rounded-lg px-3 py-2">
                      <Info className="h-3.5 w-3.5 shrink-0" />
                      <span className="shrink-0">目录：</span>
                      <code className="font-mono truncate min-w-0">{defaultDir}</code>
                    </div>
                  )}
                  {osType === "android" && androidStoragePermission && (
                    <div className={`flex items-center gap-2 text-xs rounded-lg px-3 py-2 border ${androidStoragePermission.granted
                      ? "bg-green-500/10 border-green-500/30 text-green-400"
                      : "bg-yellow-500/10 border-yellow-500/30 text-yellow-500"
                      }`}>
                      {androidStoragePermission.granted
                        ? <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
                        : <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                      }
                      <span className="min-w-0 flex-1">
                        {androidStoragePermission.message}
                      </span>
                      {!androidStoragePermission.granted && (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={handleOpenAndroidStoragePermissionSettings}
                          disabled={isCheckingAndroidStoragePermission || !androidStoragePermission.canOpenSettings}
                          className="h-7 shrink-0 gap-1.5"
                        >
                          <Settings className="h-3.5 w-3.5" />
                          去授权
                        </Button>
                      )}
                    </div>
                  )}
                  {osType === "android" && androidStoragePermissionError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>{androidStoragePermissionError}</span>
                    </div>
                  )}
                        {localSchemaInfo !== null && (
                          <div className={`flex items-center gap-2 text-xs rounded-lg px-3 py-2 border ${localSchemaInfo.installed
                            ? "bg-green-500/10 border-green-500/30 text-green-400"
                            : "bg-muted/40 border-border text-muted-foreground"
                            }`}>
                            {localSchemaInfo.installed
                              ? <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
                              : <Info className="h-3.5 w-3.5 shrink-0" />
                            }
                            <span>
                              {localSchemaInfo.installed
                                ? `已安装${localSchemaInfo.version ? ` ${localSchemaInfo.version}` : ""}${installedScheme ? ` · ${installedScheme.label}` : ""}`
                                : "未检测到已安装的键道方案"
                              }
                              {localSchemaInfo.installed && localSchemaInfo.schemas.length > 0 && (
                                <span className="ml-1 text-muted-foreground/80">({localSchemaInfo.schemas.join(", ")})</span>
                              )}
                            </span>
                          </div>
                        )}
                        {(releaseError || schemeReleaseError) && (
                          <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                            <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                            <span>获取版本信息失败：{selectedSchemeKey === "keytao" ? releaseError : schemeReleaseError}</span>
                          </div>
                        )}
                        {installError && (
                          <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                            <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                            <span>{installError}</span>
                          </div>
                        )}
                        {isInstalling && installProgress && (
                          <div className="space-y-1.5">
                            <Progress value={installProgress.percent} className="h-1.5" />
                            <p className="text-xs text-muted-foreground">{installProgress.message}</p>
                          </div>
                        )}
                        {deploySteps.length > 0 && (
                          <div className="rounded-lg border border-border bg-muted/20 px-3 py-2 space-y-1">
                      {deploySteps.map((step, i) => (
                        <div key={i} className="flex min-w-0 items-center gap-2 text-xs">
                          {step.done
                            ? <CheckCircle2 className="h-3 w-3 text-green-400 shrink-0" />
                            : step.error
                              ? <XCircle className="h-3 w-3 text-destructive shrink-0" />
                              : <Loader2 className={`h-3 w-3 shrink-0 text-muted-foreground ${isDeploying && i === deploySteps.length - 1 ? "animate-spin" : ""}`} />
                          }
                          <span className={`min-w-0 break-all ${step.error ? "text-destructive" : step.done ? "text-green-400" : "text-muted-foreground"}`}>
                            {step.msg}
                          </span>
                        </div>
                            ))}
                          </div>
                        )}
                        <div className="flex gap-2 flex-wrap">
                          <Button size="sm" onClick={handleInstall} disabled={isBusy || isFetchingSchemeRelease || !selectedSchemeDownloadUrl} className="gap-1.5">
                            {isCheckingAndroidStoragePermission || isInstalling || isDeploying
                              ? <Loader2 className="h-4 w-4 animate-spin" />
                              : <Download className="h-4 w-4" />
                            }
                            {isCheckingAndroidStoragePermission ? "检测权限..." : isInstalling ? "安装中..." : isDeploying ? "部署中..." : localSchemaInfo?.installed ? "更新方案" : "安装方案"}
                          </Button>
                          <Button variant="outline" size="sm" onClick={handleCheckLocalSchema}
                            disabled={isCheckingLocal || isBusy} className="gap-1.5">
                            <RefreshCw className={`h-3.5 w-3.5 ${isCheckingLocal ? "animate-spin" : ""}`} />
                            检查本地
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={handleOpenDefaultDir}
                            disabled={!canOpenDefaultDir || !defaultDir || isOpeningDir || isBusy}
                            className="gap-1.5"
                          >
                            <FolderOpen className="h-3.5 w-3.5" />
                            {isOpeningDir ? "打开中..." : "打开目录"}
                          </Button>
                          {canOpenRimeDictManager && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={handleOpenRimeDictManager}
                              disabled={!defaultDir || isOpeningDictManager || isBusy}
                              className="gap-1.5"
                            >
                              <BookOpen className="h-3.5 w-3.5" />
                              {isOpeningDictManager ? "打开中..." : "词库管理器"}
                            </Button>
                          )}
                          <Button variant="outline" size="sm" onClick={handleDeploy} disabled={isBusy} className="gap-1.5">
                            <Play className="h-3.5 w-3.5" />
                            部署
                          </Button>
                          {logBuffer.length > 0 && (
                            <Button variant="ghost" size="sm" onClick={() => setShowLogs(true)}
                              className="gap-1.5 text-muted-foreground ml-auto">
                              <ScrollText className="h-3.5 w-3.5" />
                              日志 ({logBuffer.length})
                            </Button>
                          )}
                        </div>
                    <textarea
                      className="w-full rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm font-mono resize-none focus:outline-none focus:ring-1 focus:ring-primary"
                      style={
                        osType === "android"
                          ? { scrollMarginBottom: "calc(var(--android-ime-inset-bottom, 0px) + 24px)" }
                          : osType === "ios"
                            ? { scrollMarginBottom: "42vh" }
                            : undefined
                      }
                      rows={3}
                      placeholder="在此测试输入法…"
                    />
                </CardContent>
              </Card>
            )}
          </div>
        )}

        {/* ══ 扩展 Tab ══════════════════════════════════════════════════════ */}
        {activeTab === "extension" && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <FolderOpen className="h-4 w-4 text-muted-foreground" />
                安装到自定义目录
                <div className="ml-auto">{VersionPicker}</div>
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <p className="text-xs text-muted-foreground">将方案安装到指定的输入法数据目录，安装完成后请手动重新部署输入法。</p>
              <div className="flex gap-2 flex-wrap">
                <Button variant="outline" size="sm" onClick={handleSelectDir} disabled={isInstallingExt} className="gap-1.5">
                  <FolderOpen className="h-4 w-4" />
                  {selectedDir ? "重新选择目录" : "选择目录"}
                </Button>
                {selectedDir && (
                  <Button variant="outline" size="sm" onClick={handleOpenSelectedDir} disabled={isOpeningExtDir} className="gap-1.5">
                    <FolderOpen className="h-3.5 w-3.5" />
                    {isOpeningExtDir ? "打开中..." : "打开目录"}
                  </Button>
                )}
                {selectedDir && selectedSchemeDownloadUrl && (
                  <Button variant="secondary" size="sm" onClick={handleInstallExt} disabled={isInstallingExt} className="gap-1.5">
                    <Download className="h-4 w-4" />
                    {isInstallingExt ? "安装中..." : "立即安装"}
                  </Button>
                )}
              </div>
              {selectedDir && (
                <div className="space-y-2">
                  <div className="flex items-center gap-2 bg-muted/40 border border-border rounded-lg px-3 py-2">
                    <CheckCircle2 className="h-4 w-4 text-green-500 shrink-0" />
                    <code className="text-xs font-mono text-muted-foreground break-all flex-1 min-w-0">{selectedDir}</code>
                  </div>
                  {localSchemas !== null && (
                    <div className="flex items-start gap-2 text-xs bg-muted/40 border border-border rounded-lg px-3 py-2">
                      <Info className="h-3.5 w-3.5 shrink-0 mt-0.5 text-muted-foreground" />
                      {localSchemas.length === 0
                        ? <span className="text-muted-foreground">未检测到 default.custom.yaml，将自动创建</span>
                        : <span className="text-muted-foreground">
                          检测到本地方案：
                          {localSchemas.map((s, i) => (
                            <span key={s}>
                              <code className="font-mono bg-muted px-1 rounded">{s}</code>
                              {i < localSchemas.length - 1 && "、"}
                            </span>
                          ))}
                        </span>
                      }
                    </div>
                  )}
                  <FileList
                    files={files}
                    loading={isLoadingFiles}
                    onRefresh={() => loadFiles(selectedDir ?? undefined, safUri ?? undefined)}
                    disabled={isInstallingExt}
                  />
                </div>
              )}
              {isInstallingExt && extProgress && (
                <div className="space-y-1.5">
                  <Progress value={extProgress.percent} className="h-1.5" />
                  <p className="text-xs text-muted-foreground">{extProgress.message}</p>
                </div>
              )}
              {extError && (
                <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2.5">
                  <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                  <span>{extError}</span>
                </div>
              )}
              {extResult && (
                <div className="rounded-lg border border-green-500/30 bg-green-500/10 px-3 py-2.5 space-y-1.5">
                  <div className="flex items-center gap-2 text-sm text-green-400">
                    <CheckCircle2 className="h-4 w-4 shrink-0" />
                    安装完成，请手动重新部署输入法
                  </div>
                  {extResult.verify.some((v) => !v.ok) && (
                    <p className="text-xs text-destructive">⚠ 有 {extResult.verify.filter((v) => !v.ok).length} 个文件校验失败</p>
                  )}
                  <details className="text-xs">
                    <summary className="cursor-pointer text-muted-foreground hover:text-foreground select-none py-1">
                      安装日志（{extResult.logs.length} 条）
                    </summary>
                    <VirtualLogViewer
                      lines={extResult.logs}
                      height={192}
                      className="mt-1 rounded-md bg-muted/60"
                      getLineClassName={logLineClassName}
                    />
                  </details>
                </div>
              )}
            </CardContent>
          </Card>
        )}

        {activeTab === "about" && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-semibold flex items-center gap-2">
                <Info className="h-4 w-4 text-muted-foreground" />
                关于
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="rounded-lg border border-border overflow-hidden">
                {[
                  ["KeyTao 版本", componentVersions?.app_version ?? (appVersion || "unknown")],
                  ["Tauri 版本", componentVersions?.tauri_version ?? "unknown"],
                  ["librime 版本", componentVersions?.librime_version ?? "unknown"],
                  ["OpenCC 版本", componentVersions?.opencc_version ?? "unknown"],
                  ["平台", osType],
                  ["键道目录", componentVersions?.data_dir ?? defaultDir ?? "unknown"],
                ].map(([label, value]) => (
                  <div key={label} className="flex items-start justify-between gap-4 px-3 py-2 border-b border-border last:border-0">
                    <span className="text-sm text-muted-foreground shrink-0">{label}</span>
                    <code className="text-xs font-mono text-right break-all">{value}</code>
                  </div>
                ))}
                <div className="flex items-center justify-between gap-4 px-3 py-2">
                  <span className="text-sm text-muted-foreground shrink-0">GitHub</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={handleOpenGitHubRepository}
                    className="h-7 gap-1.5 px-2 text-xs font-mono"
                  >
                    <GitHubIcon className="h-3.5 w-3.5" />
                    xkinput/keytao-app
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        )}

        {/* ── 日志弹窗 ──────────────────────────────────────────────────── */}
        <Dialog open={showLogs} onOpenChange={setShowLogs}>
          <DialogContent className="max-w-lg">
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <ScrollText className="h-4 w-4" />
                操作日志
              </DialogTitle>
              <DialogDescription asChild>
                <div className="space-y-2 pt-1">
                  <div className="flex justify-end">
                    <button onClick={() => setLogBuffer([])}
                      className="text-xs text-muted-foreground hover:text-destructive transition-colors">
                      清空日志
                    </button>
                  </div>
                  <div className="rounded-md bg-muted/60">
                    {logBuffer.length === 0
                      ? <p className="text-xs text-muted-foreground py-4 text-center">暂无日志</p>
                      : (
                        <VirtualLogViewer
                          lines={logBuffer}
                          height={360}
                          className="max-h-[60vh]"
                          getLineClassName={logLineClassName}
                        />
                      )}
                  </div>
                </div>
              </DialogDescription>
            </DialogHeader>
            <Button onClick={() => setShowLogs(false)} className="w-full mt-2">关闭</Button>
          </DialogContent>
        </Dialog>

        {/* ══ Debug Tab ══════════════════════════════════════════════════════ */}
        {activeTab === "debug" && (
          <Card>
            <CardContent className="pt-6">
              <DebugTab />
            </CardContent>
          </Card>
        )}

        {/* ── 更新内容弹窗 ──────────────────────────────────────────────── */}
        <Dialog open={showChangelog} onOpenChange={setShowChangelog}>
          <DialogContent className="max-w-sm">
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <FileText className="h-4 w-4" />
                {releaseInfo?.name || releaseInfo?.version} 更新内容
              </DialogTitle>
              <DialogDescription asChild>
                <div className="mt-2 max-h-96 overflow-y-auto">
                  <pre className="text-xs text-foreground/80 whitespace-pre-wrap font-sans leading-relaxed">
                    {releaseInfo?.body}
                  </pre>
                </div>
              </DialogDescription>
            </DialogHeader>
            <Button onClick={() => setShowChangelog(false)} className="w-full mt-2">关闭</Button>
          </DialogContent>
        </Dialog>

      </div>
    </div>
  )
}
