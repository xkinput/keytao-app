import { useMemo, useState } from "react"

import { cn } from "@/lib/utils"

const ROW_HEIGHT = 20
const OVERSCAN = 12

interface VirtualLogViewerProps {
  lines: string[]
  height?: number
  className?: string
  emptyText?: string
  getLineClassName?: (line: string) => string
}

export default function VirtualLogViewer({
  lines,
  height = 192,
  className,
  emptyText = "暂无日志",
  getLineClassName,
}: VirtualLogViewerProps) {
  const [scrollTop, setScrollTop] = useState(0)
  const viewportRows = Math.ceil(height / ROW_HEIGHT)
  const totalHeight = Math.max(lines.length * ROW_HEIGHT, height)
  const start = Math.max(Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN, 0)
  const end = Math.min(start + viewportRows + OVERSCAN * 2, lines.length)

  const visibleLines = useMemo(
    () => lines.slice(start, end).map((line, index) => ({
      line,
      index: start + index,
    })),
    [end, lines, start],
  )

  if (lines.length === 0) {
    return (
      <div
        className={cn(
          "flex items-center justify-center rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground",
          className,
        )}
        style={{ height }}
      >
        {emptyText}
      </div>
    )
  }

  return (
    <div
      className={cn(
        "overflow-auto rounded-lg border border-border bg-muted/40 text-left font-mono text-[11px]",
        className,
      )}
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      style={{ height }}
    >
      <div className="relative min-w-max" style={{ height: totalHeight }}>
        {visibleLines.map(({ line, index }) => (
          <div
            key={index}
            className={cn(
              "absolute left-0 right-0 whitespace-pre px-3",
              getLineClassName?.(line) ?? "text-muted-foreground",
            )}
            style={{
              height: ROW_HEIGHT,
              lineHeight: `${ROW_HEIGHT}px`,
              top: index * ROW_HEIGHT,
            }}
          >
            {line}
          </div>
        ))}
      </div>
    </div>
  )
}
