import { AlertTriangle, CheckCircle2, Keyboard, Loader2, RefreshCw, Settings } from "lucide-react"
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

interface AndroidImeOnboardingProps {
  status: AndroidImeStatus | null
  loading: boolean
  error: string | null
  onOpenSettings: () => void
  onShowPicker: () => void
  onRefresh: () => void
}

function StepRow({
  done,
  active,
  title,
  detail,
}: {
  done: boolean
  active: boolean
  title: string
  detail: string
}) {
  return (
    <div className={`flex gap-3 rounded-lg border px-3 py-3 ${active ? "border-primary/40 bg-primary/5" : "border-border bg-muted/20"}`}>
      <div className={`mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full border ${done ? "border-green-500/40 bg-green-500/15 text-green-400" : active ? "border-primary/40 text-primary" : "border-border text-muted-foreground"}`}>
        {done ? <CheckCircle2 className="h-3.5 w-3.5" /> : <Keyboard className="h-3.5 w-3.5" />}
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
  loading,
  error,
  onOpenSettings,
  onShowPicker,
  onRefresh,
}: AndroidImeOnboardingProps) {
  const enabled = status?.enabled ?? false
  const selected = status?.selected ?? false
  const ready = enabled && selected
  const progress = ready ? 100 : enabled ? 70 : status ? 38 : 12
  const primaryLabel = !enabled ? "打开系统设置" : "选择 KeyTao"
  const primaryAction = !enabled ? onOpenSettings : onShowPicker
  const primaryDisabled = loading || (!enabled ? false : !status?.canShowPicker)

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
            </div>

            {status?.message && (
              <div className="rounded-lg border border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
                {status.message}
              </div>
            )}

            {error && (
              <div className="flex items-start gap-2 rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2.5 text-sm text-destructive">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                <span>{error}</span>
              </div>
            )}

            <div className="flex gap-2">
              <Button className="flex-1 gap-1.5" onClick={primaryAction} disabled={primaryDisabled}>
                <Settings className="h-4 w-4" />
                {primaryLabel}
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
