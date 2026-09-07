import { useState, useEffect } from "react"
import { invoke } from "@tauri-apps/api/core"
import { openPath } from "@tauri-apps/plugin-opener"
import { RefreshCw, ScrollText, AlertTriangle } from "lucide-react"

import VirtualLogViewer from "@/components/VirtualLogViewer"
import { Switch } from "@/components/ui/switch"

interface DebugLogFile {
  lines: string[]
  truncated: boolean
}

interface RuntimeLogSettings {
  enabled: boolean
  level: "info" | "verbose"
  logDir: string
  files: { name: string, size: number }[]
  fileCount: number
  totalBytes: number
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

const actionClassName = "rounded bg-muted/40 px-2 py-1 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"

export default function DebugTab({ isMobile }: { isMobile: boolean }) {
  const [settings, setSettings] = useState<RuntimeLogSettings | null>(null)
  const [runtimeLogs, setRuntimeLogs] = useState<DebugLogFile>({ lines: [], truncated: false })
  const [imeLogs, setImeLogs] = useState<DebugLogFile>({ lines: [], truncated: false })
  const [appLogs, setAppLogs] = useState<DebugLogFile>({ lines: [], truncated: false })
  const [loading, setLoading] = useState(false)
  const [acting, setActing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const busy = loading || acting

  const fetchLogs = async () => {
    setLoading(true)
    setError(null)
    setNotice(null)
    try {
      const [settingsResult, runtimeResult, debugResult] = await Promise.allSettled([
        invoke<RuntimeLogSettings>("get_runtime_log_settings"),
        invoke<DebugLogFile>("read_runtime_log"),
        invoke<{ ime: DebugLogFile, app: DebugLogFile }>("read_debug_logs"),
      ])
      const errors: string[] = []
      if (settingsResult.status === "fulfilled") setSettings(settingsResult.value)
      else errors.push(`读取日志设置失败：${String(settingsResult.reason)}`)
      if (runtimeResult.status === "fulfilled") setRuntimeLogs(runtimeResult.value)
      else errors.push(`读取运行日志失败：${String(runtimeResult.reason)}`)
      if (debugResult.status === "fulfilled") {
        setImeLogs(debugResult.value.ime)
        setAppLogs(debugResult.value.app)
      } else errors.push(`读取系统日志失败：${String(debugResult.reason)}`)
      setError(errors.length ? errors.join("；") : null)
    } finally {
      setLoading(false)
    }
  }

  const runAction = async (action: () => Promise<void>, failure: string, success?: string) => {
    setActing(true)
    setError(null)
    setNotice(null)
    try {
      await action()
      if (success) setNotice(success)
    } catch (e) {
      setError(`${failure}：${String(e)}`)
    } finally {
      setActing(false)
    }
  }

  const updateSettings = (enabled: boolean, level: RuntimeLogSettings["level"]) => runAction(async () => {
    await invoke("set_runtime_log_settings", { enabled, level })
    setSettings(await invoke<RuntimeLogSettings>("get_runtime_log_settings"))
  }, "更新日志设置失败")

  const copyRecentLogs = () => runAction(async () => {
    const logs = await invoke<DebugLogFile>("read_runtime_log", { maxLines: 200 })
    await navigator.clipboard.writeText(logs.lines.join("\n"))
  }, "复制日志失败", "已复制最近的运行日志（最多 200 行）")

  const openLogDirectory = () => settings && runAction(async () => {
    try {
      await openPath(settings.logDir)
    } catch {
      // Resolve the same fixed log directory through the native opener plugin.
      await invoke("share_runtime_log")
    }
  }, "打开日志目录失败")

  const clearLogs = () => runAction(async () => {
    await invoke("clear_runtime_log")
    await fetchLogs()
  }, "清空运行日志失败", "已清空运行日志")

  useEffect(() => {
    fetchLogs()
  }, [])

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold flex items-center gap-2">
          <ScrollText className="h-4 w-4" />
          运行日志
        </h2>
        <div className="flex flex-wrap items-center gap-3 text-xs">
          <label className="flex items-center gap-2" htmlFor="runtime-log-enabled">
            <Switch
              id="runtime-log-enabled"
              checked={settings?.enabled ?? false}
              disabled={busy || !settings}
              onCheckedChange={(enabled) => settings && updateSettings(enabled, settings.level)}
            />
            {settings ? (settings.enabled ? "已开启" : "已关闭") : "读取设置中"}
          </label>
          <label className="flex items-center gap-2">
            级别
            <select
              value={settings?.level ?? "info"}
              disabled={busy || !settings}
              onChange={(event) => settings && updateSettings(settings.enabled, event.target.value === "verbose" ? "verbose" : "info")}
              className="rounded border border-border bg-background px-2 py-1 disabled:opacity-50"
            >
              <option value="info">info</option>
              <option value="verbose">verbose</option>
            </select>
          </label>
          <button
            onClick={fetchLogs}
            disabled={busy}
            className={`${actionClassName} flex items-center gap-1`}
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
            刷新
          </button>
        </div>
      </div>

      {error && (
        <div role="alert" className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
          <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
          <span className="min-w-0 break-words">{error}</span>
        </div>
      )}
      {notice && <p role="status" className="text-xs text-muted-foreground">{notice}</p>}

      <div className="space-y-2">
        {settings && (
          <div className="space-y-1 text-xs text-muted-foreground">
            <p>{settings.fileCount} 个文件 · 共 {formatBytes(settings.totalBytes)}</p>
            {settings.files.map((file) => (
              <p key={file.name} className="break-all">{file.name} · {formatBytes(file.size)}</p>
            ))}
            {!isMobile && <p className="break-all">{settings.logDir}</p>}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-2">
          {isMobile ? (
            <button
              onClick={() => runAction(() => invoke("share_runtime_log"), "分享失败，可尝试复制最近 200 行")}
              disabled={busy}
              className={actionClassName}
            >
              分享
            </button>
          ) : (
            <>
              <button
                onClick={openLogDirectory}
                disabled={busy || !settings}
                className={actionClassName}
              >
                打开日志目录
              </button>
              <button
                onClick={() => settings && runAction(() => navigator.clipboard.writeText(settings.logDir), "复制路径失败", "已复制日志目录路径")}
                disabled={busy || !settings}
                className={actionClassName}
              >
                复制路径
              </button>
            </>
          )}
          <button onClick={copyRecentLogs} disabled={busy} className={actionClassName}>
            复制最近 200 行
          </button>
          <button onClick={clearLogs} disabled={busy} className={actionClassName}>
            清空
          </button>
        </div>
        <h3 className="text-xs font-semibold text-muted-foreground">
          结构化运行日志
          {runtimeLogs.truncated && <span className="ml-2 font-normal">仅显示最近 {runtimeLogs.lines.length} 行</span>}
        </h3>
        <VirtualLogViewer lines={runtimeLogs.lines} height={240} />
      </div>

      <div className="space-y-2">
        <h3 className="text-xs font-semibold text-muted-foreground">
          keytao-ime (系统服务进程)
          {imeLogs.truncated && <span className="ml-2 font-normal">仅显示最近 {imeLogs.lines.length} 行</span>}
        </h3>
        <VirtualLogViewer lines={imeLogs.lines} height={192} />
      </div>

      <div className="space-y-2">
        <h3 className="text-xs font-semibold text-muted-foreground">
          keytao-app (当前界面进程)
          {appLogs.truncated && <span className="ml-2 font-normal">仅显示最近 {appLogs.lines.length} 行</span>}
        </h3>
        <VirtualLogViewer lines={appLogs.lines} height={192} />
      </div>
    </div>
  )
}
