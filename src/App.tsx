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
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog"
import VirtualLogViewer from "@/components/VirtualLogViewer"
import {
  FolderOpen,
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
  type LucideIcon,
} from "lucide-react"
import DebugTab from "@/components/DebugTab"

type OSType = "windows" | "macos" | "linux" | "android" | "ios" | "unknown"
type Tab = "ime" | "extension" | "about" | "debug"

const GITHUB_REPOSITORY_URL = "https://github.com/xkinput/keytao-app"

function hasSystemIme(os: OSType): boolean {
  return os === "linux" || os === "macos" || os === "windows"
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
  command: string
  processes: string[]
  kde_native_processes: number
  fallback_processes: number
  message: string
}

interface WindowsImeStatus {
  supported: boolean
  packaged: boolean
  registered: boolean
  runtime_dir: string | null
  dll_path: string | null
  registered_path: string | null
  message: string
}

interface MacosImeStatus {
  installed: boolean
  app_path: string | null
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

  // Linux IME daemon
  const [linuxImeStatus, setLinuxImeStatus] = useState<LinuxImeStatus | null>(null)
  const [linuxImeError, setLinuxImeError] = useState<string | null>(null)
  const [windowsImeStatus, setWindowsImeStatus] = useState<WindowsImeStatus | null>(null)
  const [windowsImeError, setWindowsImeError] = useState<string | null>(null)
  const [macosImeStatus, setMacosImeStatus] = useState<MacosImeStatus | null>(null)
  const [macosImeError, setMacosImeError] = useState<string | null>(null)

  // Default data dir
  const [defaultDir, setDefaultDir] = useState<string | null>(null)

  // Local schema info
  const [localSchemaInfo, setLocalSchemaInfo] = useState<LocalSchemaInfo | null>(null)
  const [isCheckingLocal, setIsCheckingLocal] = useState(false)
  const [isOpeningDir, setIsOpeningDir] = useState(false)
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

  function addLogs(lines: string[]) {
    const ts = new Date().toLocaleTimeString()
    setLogBuffer((prev) => [...prev, ...lines.map((l) => `[${ts}] ${l}`)])
  }

  useEffect(() => {
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

    invoke<string | null>("rime_get_data_dir")
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
      invoke<WindowsImeStatus>("windows_ime_status")
        .then(setWindowsImeStatus)
        .catch((e) => setWindowsImeError(String(e)))
    }
    if (os === "macos") {
      invoke<MacosImeStatus>("macos_ime_status")
        .then(setMacosImeStatus)
        .catch((e) => setMacosImeError(String(e)))
    }

    listen<InstallProgress>("install-progress", (e) => {
      setInstallProgress(e.payload)
      setExtProgress(e.payload)
    }).then((fn) => { unlistenInstallRef.current = fn })

    return () => {
      unlistenInstallRef.current?.()
      unlistenDeployRef.current?.()
    }
  }, [])

  const activePlatform = downloadSource === "gitee" ? releaseInfo?.gitee : releaseInfo?.github
  const downloadUrl = activePlatform?.download_urls?.[osType as keyof PlatformRelease["download_urls"]]
  const isBusy = isInstalling || isDeploying
  const systemImeAvailable = hasSystemIme(osType)

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

  async function handleInstall() {
    if (!downloadUrl) return
    setIsInstalling(true)
    setInstallProgress(null)
    setInstallError(null)
    setDeploySteps([])

    try {
      const result = await invoke<InstallResult>("rime_install_to_default", { url: downloadUrl })
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
    setIsFetchingRelease(true)
    setReleaseError(null)
    invoke<ReleaseInfo>("fetch_latest_release")
      .then(setReleaseInfo)
      .catch((e) => setReleaseError(String(e)))
      .finally(() => setIsFetchingRelease(false))
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
    if (!selectedDir || !downloadUrl) return
    setIsInstallingExt(true)
    setExtResult(null)
    setExtError(null)
    setExtProgress(null)
    try {
      const tempPath = await invoke<string>("download_to_temp", { url: downloadUrl })
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
      {releaseInfo?.github && (
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
      {releaseInfo && !releaseInfo.github && (
        <Badge variant="secondary" className="font-mono text-xs">{releaseInfo.version}</Badge>
      )}
      {releaseInfo?.body && (
        <button
          onClick={() => setShowChangelog(true)}
          className="text-xs text-muted-foreground hover:text-foreground transition-colors underline underline-offset-2"
        >
          更新内容
        </button>
      )}
      {isFetchingRelease
        ? <RefreshCw className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
        : <Button variant="ghost" size="icon" className="h-6 w-6" onClick={handleRefetchRelease} title="检查新版本">
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
      }
    </div>
  )

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="max-w-2xl mx-auto px-4 py-6 space-y-4">

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
                      {windowsImeStatus?.registered
                        ? <Badge className="text-xs gap-1 bg-green-500/20 text-green-400 border-green-500/30"><CheckCircle2 className="h-3 w-3" />已注册</Badge>
                        : <Badge variant="outline" className="text-xs">未注册</Badge>
                      }
                    </span>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {windowsImeStatus && (
                    <div className="grid gap-2 text-xs">
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge variant={windowsImeStatus.packaged ? "default" : "outline"} className="text-xs">
                          安装包 {windowsImeStatus.packaged ? "完整" : "缺少 IME 运行时"}
                        </Badge>
                        <Badge variant={windowsImeStatus.registered ? "default" : "outline"} className="text-xs">
                          TSF {windowsImeStatus.registered ? "已注册" : "未注册"}
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
                      {windowsImeStatus.message && (
                        <div className="text-xs text-muted-foreground rounded-lg border border-border bg-muted/20 px-3 py-2">
                          {windowsImeStatus.message}
                        </div>
                      )}
                    </div>
                  )}
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
                    <Download className="h-4 w-4 text-muted-foreground" />
                    键道方案
                    <div className="ml-auto">{VersionPicker}</div>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  {defaultDir && (
                    <div className="flex items-center gap-2 text-xs text-muted-foreground bg-muted/40 border border-border rounded-lg px-3 py-2">
                      <Info className="h-3.5 w-3.5 shrink-0" />
                      <span>目录：<code className="font-mono">{defaultDir}</code></span>
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
                          ? `已安装${localSchemaInfo.version ? ` ${localSchemaInfo.version}` : ""}`
                          : "未检测到已安装的键道方案"
                        }
                        {localSchemaInfo.installed && localSchemaInfo.schemas.length > 0 && (
                          <span className="ml-1 text-muted-foreground/80">({localSchemaInfo.schemas.join(", ")})</span>
                        )}
                      </span>
                    </div>
                  )}
                  {releaseError && (
                    <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
                      <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                      <span>获取版本信息失败：{releaseError}</span>
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
                        <div key={i} className="flex items-center gap-2 text-xs">
                          {step.done
                            ? <CheckCircle2 className="h-3 w-3 text-green-400 shrink-0" />
                            : step.error
                              ? <XCircle className="h-3 w-3 text-destructive shrink-0" />
                              : <Loader2 className={`h-3 w-3 shrink-0 text-muted-foreground ${isDeploying && i === deploySteps.length - 1 ? "animate-spin" : ""}`} />
                          }
                          <span className={step.error ? "text-destructive" : step.done ? "text-green-400" : "text-muted-foreground"}>
                            {step.msg}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="flex gap-2 flex-wrap">
                    <Button size="sm" onClick={handleInstall} disabled={isBusy || !downloadUrl} className="gap-1.5">
                      <Download className="h-4 w-4" />
                      {isInstalling ? "安装中..." : isDeploying ? "部署中..." : localSchemaInfo?.installed ? "更新方案" : "安装方案"}
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
                      disabled={!defaultDir || isOpeningDir || isBusy}
                      className="gap-1.5"
                    >
                      <FolderOpen className="h-3.5 w-3.5" />
                      {isOpeningDir ? "打开中..." : "打开目录"}
                    </Button>
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
                {selectedDir && downloadUrl && (
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
