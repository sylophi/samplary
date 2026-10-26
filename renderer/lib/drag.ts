import { api, type Row } from "@/lib/api";
import { dragImage, peekThumbnail } from "@/lib/waveform";

let outgoing: string | null = null;

/** Hands the sample's file to the OS, to drop into a DAW or Finder. */
export function dragOut(row: Row, dark: boolean) {
  outgoing = row.path;
  void api.startDrag(row.path, dragImage(row.name, peekThumbnail(row.id), dark));
}

// A native drag says nothing when it ends. The pointer moving with no
// button held means it has.
window.addEventListener("mousemove", (e) => {
  if (e.buttons === 0) outgoing = null;
});

/** Whether a sample is being dragged out of this window. */
export function isOutgoing() {
  return outgoing !== null;
}

/**
 * Tells a click from the start of a drag: `onDrag` fires once the pointer
 * travels a few pixels with the button down, `onClick` if it never does.
 */
export function pressOrDrag(
  down: React.MouseEvent,
  { onClick, onDrag }: { onClick?: (e: MouseEvent) => void; onDrag: () => void },
) {
  if (down.button !== 0) return;
  const x = down.clientX;
  const y = down.clientY;
  const move = (e: MouseEvent) => {
    if (Math.hypot(e.clientX - x, e.clientY - y) < 5) return;
    cleanup();
    onDrag();
  };
  const up = (e: MouseEvent) => {
    cleanup();
    onClick?.(e);
  };
  const cleanup = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}
