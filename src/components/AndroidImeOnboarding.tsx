import { AlertTriangle, CheckCircle2, Download, FolderOpen, Keyboard, Loader2, RefreshCw, Settings } from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"

export interface AndroidImeStatus {
  packageName: string
  serviceName: string
  inputMethodId: string | null
  defaultInputMethod: string | null
  enabled: boolean
  selected: boolean
  canShowPicker: boolean
  message: string
}

export interface AndroidStoragePermissionStatus {
  path: string
  granted: boolean
  writable: boolean
  requiresManageAllFiles: boolean
  canOpenSettings: boolean
  message: string
}

interface AndroidImeOnboardingProps {
  status: AndroidImeStatus | null
  storageStatus: AndroidStoragePermissionStatus | null
  schemaInstalled: boolean
  loading: boolean
  error: string | null
  storageError: string | null
  installError: string | null
  installingSchema: boolean
  canInstallSchema: boolean
  onOpenSettings: () => void
  onShowPicker: () => void
  onOpenStorageSettings: () => void
  onInstallSchema: () => void
  onRefresh: () => void
}

function StepRow({
  done,
  active,
  icon,
  title,
  detail,
}: {
  done: boolean
  active: boolean
  icon?: "keyboard" | "folder" | "download"
  title: string
  detail: string
}) {
  const Icon = icon === "folder" ? FolderOpen : icon === "download" ? Download : Keyboard
  return (
    <div className={`flex gap-3 rounded-lg border px-3 py-3 ${active ? "border-primary/40 bg-primary/5" : "border-border bg-muted/20"}`}>
      <div className={`mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full border ${done ? "border-green-500/40 bg-green-500/15 text-green-400" : active ? "border-primary/40 text-primary" : "border-border text-muted-foreground"}`}>
        {done ? <CheckCircle2 className="h-3.5 w-3.5" /> : <Icon className="h-3.5 w-3.5" />}
      </div>
      <div className="min-w-0 space-y-1">
        <div className="text-sm font-medium leading-none">{title}</div>
        <p className="text-xs leading-relaxed text-muted-foreground">{detail}</p>
      </div>
    </div>
  )
}

export default function AndroidImeOnboarding({
  status,
  storageStatus,
  schemaInstalled,
  loading,
  error,
  storageError,
  installError,
  installingSchema,
  canInstallSchema,
  onOpenSettings,
  onShowPicker,
  onOpenStorageSettings,
  onInstallSchema,
  onRefresh,
}: AndroidImeOnboardingProps) {
  const enabled = status?.enabled ?? false
  const selected = status?.selected ?? false
  const storageGranted = storageStatus?.granted ?? false
  const ready = enabled && selected && storageGranted && schemaInstalled
  const progress = ready ? 100 : schemaInstalled ? 92 : storageGranted ? 75 : selected ? 58 : enabled ? 38 : status ? 18 : 8
  const permissionDetail = storageStatus?.path
    ? `授权后 KeyTao 才能写入 ${storageStatus.path} 并让输入法读取方案。`
    : "授权后 KeyTao 才能写入用户目录并让输入法读取方案。"

  const primary = !enabled
    ? { label: "打开系统设置", icon: Settings, action: onOpenSettings, disabled: false }
    : !selected
      ? { label: "选择 KeyTao", icon: Keyboard, action: onShowPicker, disabled: !status?.canShowPicker }
      : !storageGranted
        ? { label: "授权文件访问", icon: FolderOpen, action: onOpenStorageSettings, disabled: storageStatus ? !storageStatus.canOpenSettings : false }
        : !schemaInstalled
          ? { label: installingSchema ? "安装中..." : "安装键道方案", icon: Download, action: onInstallSchema, disabled: !canInstallSchema || installingSchema }
          : { label: "重新检测", icon: RefreshCw, action: onRefresh, disabled: false }
  const PrimaryIcon = primary.icon
  const primaryDisabled = loading || primary.disabled

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="mx-auto flex min-h-screen max-w-md flex-col justify-center px-4 py-8">
        <div className="mb-5 flex items-center gap-3">
          <img src="/logo.png" alt="KeyTao" className="h-11 w-11" />
          <div className="min-w-0">
            <h1 className="text-lg font-semibold tracking-tight">KeyTao 键道</h1>
            <p className="text-xs text-muted-foreground">启用 Android 输入法</p>
          </div>
          <Badge variant={ready ? "default" : "outline"} className="ml-auto shrink-0 text-xs">
            {ready ? "已就绪" : "待配置"}
          </Badge>
        </div>

        <Card className="overflow-hidden">
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-sm">
              <Settings className="h-4 w-4 text-muted-foreground" />
              输入法设置
              {loading && <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <Progress value={progress} className="h-1.5" />

            <div className="space-y-2">
              <StepRow
                done={enabled}
                active={!enabled}
                title="启用 KeyTao"
                detail="系统设置中打开 KeyTao 输入法开关。"
              />
              <StepRow
                done={selected}
                active={enabled && !selected}
                title="切换到 KeyTao"
                detail="从系统输入法选择器中选中 KeyTao。"
              />
              <StepRow
                done={storageGranted}
                active={enabled && selected && !storageGranted}
                icon="folder"
                title="授予文件访问权限"
                detail={permissionDetail}
              />
              <StepRow
                done={schemaInstalled}
                active={enabled && selected && storageGranted && !schemaInstalled}
                icon="download"
                title="安装键道方案"
                detail="安装完成后候选条会加载键道方案，不再提示先授权或先安装。"
              />
            </div>

            {status?.message && (
              <div className="rounded-lg border border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
                {status.message}
              </div>
            )}

            {storageStatus?.message && (
              <div className="rounded-lg border border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
                {storageStatus.message}
              </div>
            )}

            {(error || storageError || installError) && (
              <div className="flex items-start gap-2 rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2.5 text-sm text-destructive">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                <span>{error || storageError || installError}</span>
              </div>
            )}

            <div className="flex gap-2">
              <Button className="flex-1 gap-1.5" onClick={primary.action} disabled={primaryDisabled}>
                <PrimaryIcon className={`h-4 w-4 ${installingSchema && !schemaInstalled ? "animate-spin" : ""}`} />
                {primary.label}
              </Button>
              <Button variant="outline" size="icon" onClick={onRefresh} disabled={loading} title="重新检测">
                <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
