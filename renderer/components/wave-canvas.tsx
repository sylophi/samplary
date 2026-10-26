import { useEffect, useRef, useState } from "react";

import { drawWave, useDark, type WaveData } from "@/lib/waveform";

/**
 * A waveform at its element's size. Pass width and height when they're
 * fixed to skip measuring.
 */
export function WaveCanvas({
  data,
  width,
  height,
  className,
}: {
  data: WaveData | null;
  width?: number;
  height?: number;
  className?: string;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const dark = useDark();
  const [measured, setMeasured] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || (width && height)) return;
    const observer = new ResizeObserver(([entry]) => {
      const box = entry.contentRect;
      setMeasured({ w: box.width, h: box.height });
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [width, height]);

  const w = width ?? measured?.w ?? 0;
  const h = height ?? measured?.h ?? 0;
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || !w || !h) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    if (data) drawWave(canvas, data, dark);
  }, [data, dark, w, h]);

  return (
    <canvas
      ref={ref}
      className={className}
      style={width && height ? { width, height } : undefined}
    />
  );
}
