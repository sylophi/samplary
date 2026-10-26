import type { Row } from "@/lib/api";

export function formatDuration(seconds: number) {
  if (seconds < 1) return `${Math.round(seconds * 1000)} ms`;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 2 : 1)} s`;
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function formatCount(n: number) {
  return n.toLocaleString("en-US");
}

export function formatTempo(bpm: number) {
  return Math.abs(bpm - Math.round(bpm)) < 0.05 ? String(Math.round(bpm)) : bpm.toFixed(1);
}

/** The last two folders of a path: enough to place a sample. */
export function shortFolder(folder: string) {
  return folder.split("/").slice(-2).join("/");
}

/** The note a one-shot sounds at, or the key a loop is in. */
export function pitchLabel(row: Row) {
  return row.note ?? row.key ?? null;
}

export function formatFormat(row: Row) {
  const rate = `${(row.sampleRate / 1000).toFixed(row.sampleRate % 1000 === 0 ? 0 : 1)} kHz`;
  const bits = row.bits ? `${row.bits}-bit` : null;
  const channels =
    row.channels === 1 ? "mono" : row.channels === 2 ? "stereo" : `${row.channels}-channel`;
  return [bits, rate, channels].filter(Boolean).join(" ");
}
