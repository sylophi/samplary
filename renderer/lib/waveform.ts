import { useEffect, useState, useSyncExternalStore } from "react";

import { api, BYTES_PER_COLUMN, THUMB_COLUMNS } from "@/lib/api";

/**
 * Columns of [min, max, low, mid, high]: signed peak extents (the loudest
 * is 127), then the balance of low, mid and high frequencies (the loudest
 * band is 255).
 */
export type WaveData = Uint8Array;

const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

export function useDark() {
  return useSyncExternalStore(
    (onChange) => {
      darkQuery.addEventListener("change", onChange);
      return () => darkQuery.removeEventListener("change", onChange);
    },
    () => darkQuery.matches,
  );
}

/**
 * Red for lows, green for mids, blue for highs, as in songloupe's spectral
 * waveform: kicks glow red, hats blue, full-range sounds pale. Softened so
 * a single band reads as a hue rather than a raw primary.
 */
function color(r: number, g: number, b: number, dark: boolean) {
  const soft = (c: number) => 0.3 + 0.7 * (c / 255) ** 0.75;
  if (dark) {
    return `rgb(${soft(r) * 240},${soft(g) * 232},${soft(b) * 248})`;
  }
  return `rgb(${soft(r) * 200 - 20},${soft(g) * 170 - 20},${soft(b) * 210 - 10})`;
}

export function drawWave(canvas: HTMLCanvasElement, data: WaveData, dark: boolean) {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const { width, height } = canvas;
  ctx.clearRect(0, 0, width, height);
  const columns = Math.floor(data.length / BYTES_PER_COLUMN);
  if (columns === 0) return;
  const mid = height / 2;
  const half = height / 2 - 0.5;
  for (let x = 0; x < width; x++) {
    const c0 = Math.floor((x * columns) / width);
    const c1 = Math.max(c0 + 1, Math.floor(((x + 1) * columns) / width));
    let lo = 0;
    let hi = 0;
    let r = 0;
    let g = 0;
    let b = 0;
    for (let c = c0; c < c1; c++) {
      const o = c * BYTES_PER_COLUMN;
      lo = Math.min(lo, (data[o] << 24) >> 24);
      hi = Math.max(hi, (data[o + 1] << 24) >> 24);
      r += data[o + 2];
      g += data[o + 3];
      b += data[o + 4];
    }
    const n = c1 - c0;
    const top = mid - (hi / 127) * half;
    const bottom = mid - (lo / 127) * half;
    ctx.fillStyle = color(r / n, g / n, b / n, dark);
    ctx.fillRect(x, top, 1, Math.max(1, bottom - top));
  }
}

// Thumbnails, fetched in batches for whatever rows are on screen.
const thumbs = new Map<number, WaveData>();
const wanted = new Set<number>();
const thumbListeners = new Set<() => void>();
let flushTimer = 0;

function flushThumbs() {
  flushTimer = 0;
  const ids = [...wanted];
  wanted.clear();
  if (ids.length === 0) return;
  const size = THUMB_COLUMNS * BYTES_PER_COLUMN;
  void api.thumbnails(ids).then((bytes) => {
    ids.forEach((id, i) => thumbs.set(id, bytes.slice(i * size, (i + 1) * size)));
    for (const l of thumbListeners) l();
  });
}

export function clearThumbnails() {
  thumbs.clear();
  for (const l of thumbListeners) l();
}

export function useThumbnail(id: number): WaveData | null {
  const [, bump] = useState(0);
  useEffect(() => {
    const listener = () => bump((n) => n + 1);
    thumbListeners.add(listener);
    return () => {
      thumbListeners.delete(listener);
    };
  }, []);
  const data = thumbs.get(id) ?? null;
  if (!data && !wanted.has(id)) {
    wanted.add(id);
    if (!flushTimer) flushTimer = window.setTimeout(flushThumbs, 16);
  }
  return data;
}

export function peekThumbnail(id: number) {
  return thumbs.get(id) ?? null;
}

/** A PNG of the sample's waveform and name, to carry along when dragged out. */
export function dragImage(name: string, data: WaveData | null, dark: boolean) {
  const scale = 2;
  const canvas = document.createElement("canvas");
  canvas.width = 220 * scale;
  canvas.height = 52 * scale;
  const ctx = canvas.getContext("2d")!;
  ctx.scale(scale, scale);
  ctx.fillStyle = dark ? "#262626" : "#ffffff";
  ctx.strokeStyle = dark ? "rgba(255,255,255,0.12)" : "rgba(0,0,0,0.12)";
  ctx.beginPath();
  ctx.roundRect(0.5, 0.5, 219, 51, 8);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = dark ? "#fafafa" : "#171717";
  ctx.font = "500 12px 'Geist Variable', system-ui";
  let label = name;
  if (ctx.measureText(label).width > 196) {
    while (label.length > 1 && ctx.measureText(`${label}…`).width > 196) {
      label = label.slice(0, -1);
    }
    label += "…";
  }
  ctx.fillText(label, 12, 19);
  if (data) {
    const wave = document.createElement("canvas");
    wave.width = 196 * scale;
    wave.height = 18 * scale;
    drawWave(wave, data, dark);
    ctx.drawImage(wave, 12, 26, 196, 18);
  }
  return canvas.toDataURL("image/png");
}
