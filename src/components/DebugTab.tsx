import { useState, useEffect } from "react"
import { invoke } from "@tauri-apps/api/core"
import { RefreshCw, ScrollText, AlertTriangle } from "lucide-react"

import VirtualLogViewer from "@/components/VirtualLogViewer"

interface DebugLogFile {
  lines: string[]
  truncated: boolean
}

export default function DebugTab() {
  const [imeLogs, setImeLogs] = useState<DebugLogFile>({ lines: [], truncated: false })
  const [appLogs, setAppLogs] = useState<DebugLogFile>({ lines: [], truncated: false })
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const fetchLogs = async () => {
    setLoading(true)
    setError(null)
    try {
      const logs: { ime: DebugLogFile, app: DebugLogFile } = await invoke("read_debug_logs")
      setImeLogs(logs.ime)
      setAppLogs(logs.app)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    fetchLogs()
  }, [])

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold flex items-center gap-2">
          <ScrollText className="h-4 w-4" />
          运行日志
        </h2>
        <button
          onClick={fetchLogs}
          disabled={loading}
          className="text-xs text-muted-foreground hover:text-foreground flex items-center gap-1 bg-muted/40 px-2 py-1 rounded"
        >
          <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
          刷新
        </button>
      </div>

      {error && (
        <div className="flex items-start gap-2 text-sm text-destructive bg-destructive/10 border border-destructive/20 rounded-lg px-3 py-2">
          <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
          <span>读取日志失败：{error}</span>
        </div>
      )}

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
