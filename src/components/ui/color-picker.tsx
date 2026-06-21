import { useEffect, useState, type CSSProperties } from "react"
import { Sketch } from "@uiw/react-color"
import { Check, Paintbrush } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { cn } from "@/lib/utils"

interface ColorPickerProps {
  value: string
  onChange: (value: string) => void
  disabled?: boolean
  presets?: string[]
  className?: string
  title?: string
}

const HEX_COLOR_PATTERN = /^#[0-9a-fA-F]{6}$/

function normalizeHexColor(color: string) {
  const trimmed = color.trim()
  const withHash = trimmed.startsWith("#") ? trimmed : `#${trimmed}`
  return HEX_COLOR_PATTERN.test(withHash) ? withHash.toUpperCase() : null
}

function ColorPicker({
  value,
  onChange,
  disabled = false,
  presets = [],
  className,
  title = "选择颜色",
}: ColorPickerProps) {
  const [open, setOpen] = useState(false)
  const [draft, setDraft] = useState(value)
  const presetColors = presets.slice(0, 4)

  useEffect(() => {
    if (!open) {
      setDraft(value)
    }
  }, [open, value])

  const commitColor = (color: string) => {
    const normalized = normalizeHexColor(color)
    if (!normalized) {
      setDraft(value)
      return
    }
    setDraft(normalized)
    if (normalized.toLowerCase() !== value.toLowerCase()) {
      onChange(normalized)
    }
  }

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen && open) {
      commitColor(draft)
    }
    if (nextOpen) {
      setDraft(value)
    }
    setOpen(nextOpen)
  }

  const handleCancel = () => {
    setDraft(value)
    setOpen(false)
  }

  const handleApply = () => {
    commitColor(draft)
    setOpen(false)
  }

  return (
    <div className={cn("flex min-w-0 items-center justify-end gap-2", className)}>
      {presetColors.length > 0 && (
        <div className="grid shrink-0 grid-cols-2 gap-1">
          {presetColors.map((color) => {
            const selected = color.toLowerCase() === value.toLowerCase()
            return (
              <button
                key={color}
                type="button"
                disabled={disabled}
                onClick={() => commitColor(color)}
                className={cn(
                  "h-6 w-6 rounded-md border border-white/15 shadow-sm transition-transform hover:scale-105 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50",
                  selected && "ring-2 ring-ring ring-offset-2 ring-offset-background",
                )}
                style={{ backgroundColor: color }}
                aria-label={`使用颜色 ${color}`}
              >
                {selected && <Check className="mx-auto h-3.5 w-3.5 text-white drop-shadow" />}
              </button>
            )
          })}
        </div>
      )}
      <Popover open={open} onOpenChange={handleOpenChange}>
        <PopoverTrigger asChild>
          <button
            type="button"
            disabled={disabled}
            className={cn(
              "inline-flex h-10 w-14 shrink-0 items-center justify-center rounded-lg border border-border bg-background shadow-sm transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50",
              open && "bg-accent",
            )}
            title={title}
            aria-label={title}
          >
            <span
              className="grid h-7 w-7 place-items-center rounded-md border border-white/20 shadow-inner"
              style={{ backgroundColor: open ? draft : value }}
            >
              <Paintbrush className="h-4 w-4 text-white drop-shadow" />
            </span>
          </button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-auto p-3">
          <div className="space-y-3">
            <Sketch
              color={draft}
              disableAlpha
              editableDisable
              presetColors={false}
              width={224}
              onChange={(color) => setDraft(color.hex)}
              style={
                {
                  "--sketch-background": "hsl(var(--card))",
                  "--sketch-box-shadow": "none",
                  "--sketch-alpha-box-shadow": "hsl(var(--border)) 0 0 0 1px inset",
                  "--editable-input-box-shadow": "hsl(var(--border)) 0 0 0 1px inset",
                  "--editable-input-color": "hsl(var(--foreground))",
                  "--editable-input-label-color": "hsl(var(--muted-foreground))",
                } as CSSProperties
              }
            />
            <div className="flex items-center justify-between gap-3">
              <code className="rounded-md border border-border bg-background px-2 py-1 font-mono text-xs uppercase text-muted-foreground">
                {draft}
              </code>
              <div className="flex items-center gap-1.5">
                <Button type="button" variant="ghost" size="sm" onClick={handleCancel}>
                  取消
                </Button>
                <Button type="button" size="sm" onClick={handleApply}>
                  应用
                </Button>
              </div>
            </div>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  )
}

export { ColorPicker }
