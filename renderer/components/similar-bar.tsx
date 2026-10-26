import { ScanSearch, X } from "lucide-react";

import { WaveCanvas } from "@/components/wave-canvas";
import type { Row, Weights } from "@/lib/api";
import { useThumbnail } from "@/lib/waveform";

const ASPECTS = [
  { label: "spectrum", title: "Overall frequency balance: dark or bright, full or thin" },
  { label: "timbre", title: "Tone color and texture, at the attack and through the sound" },
  { label: "pitch", title: "Fundamental, how it moves, and which notes are present" },
  { label: "amplitude", title: "Loudness over time: attack, decay, length, rhythm" },
];

export function SimilarBar({
  reference,
  weights,
  onWeights,
  onSelect,
  onClose,
}: {
  reference: Row;
  weights: Weights;
  onWeights: (weights: Weights) => void;
  onSelect: () => void;
  onClose: () => void;
}) {
  const thumb = useThumbnail(reference.id);
  return (
    <div className="flex h-11 shrink-0 items-center gap-3 border-b px-4 text-[13px]">
      <ScanSearch className="size-4 shrink-0 text-muted-foreground" />
      <span className="shrink-0 text-muted-foreground">sounds like</span>
      <button
        onClick={onSelect}
        title="Select it"
        className="flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1 hover:bg-accent"
      >
        <WaveCanvas data={thumb} width={56} height={18} />
        <span className="truncate font-medium">{reference.name}</span>
      </button>
      <div className="ml-auto flex min-w-0 items-center gap-4">
        {ASPECTS.map((aspect, i) => (
          <label
            key={aspect.label}
            title={aspect.title}
            className="flex min-w-0 items-center gap-2"
          >
            <span className={weights[i] > 0 ? "" : "text-muted-foreground"}>{aspect.label}</span>
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={weights[i]}
              style={{ "--fill": `${weights[i] * 100}%` } as React.CSSProperties}
              onChange={(e) => {
                const next: [number, number, number, number] = [...weights];
                next[i] = Number(e.target.value);
                onWeights(next);
              }}
              className="w-20 min-w-0"
            />
          </label>
        ))}
        <button
          title="Back to browsing (esc)"
          onClick={onClose}
          className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <X className="size-4" />
        </button>
      </div>
    </div>
  );
}
