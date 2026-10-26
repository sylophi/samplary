import { Search, Volume1, Volume2, VolumeX, X } from "lucide-react";

import type { Kind } from "@/lib/api";
import { player, useVolume } from "@/lib/audio";

const KINDS: { value: Kind | null; label: string }[] = [
  { value: null, label: "all" },
  { value: "oneShot", label: "one-shots" },
  { value: "loop", label: "loops" },
];

export function Toolbar({
  text,
  onText,
  inputRef,
  kind,
  onKind,
}: {
  text: string;
  onText: (text: string) => void;
  inputRef: React.RefObject<HTMLInputElement | null>;
  kind: Kind | null;
  onKind: (kind: Kind | null) => void;
}) {
  const volume = useVolume();
  const VolumeIcon = volume === 0 ? VolumeX : volume < 0.5 ? Volume1 : Volume2;
  return (
    <header data-drag-region className="flex h-13 shrink-0 items-center gap-3 border-b px-4">
      <div className="relative w-80 shrink">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <input
          ref={inputRef}
          value={text}
          onChange={(e) => onText(e.target.value)}
          placeholder="search names and folders"
          spellCheck={false}
          className="h-8 w-full rounded-md border bg-transparent pr-7 pl-8 text-[13px] outline-none placeholder:text-muted-foreground focus:border-ring"
        />
        {text && (
          <button
            title="Clear"
            onClick={() => onText("")}
            className="absolute top-1/2 right-1.5 flex size-5 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:text-foreground"
          >
            <X className="size-3.5" />
          </button>
        )}
      </div>
      <div className="flex h-8 shrink-0 items-center rounded-md border p-0.5 text-[13px]">
        {KINDS.map((k) => (
          <button
            key={k.label}
            onClick={() => onKind(k.value)}
            className={`h-full rounded px-2.5 ${
              kind === k.value ? "bg-selected" : "text-muted-foreground hover:text-foreground"
            }`}
          >
            {k.label}
          </button>
        ))}
      </div>
      <div data-drag-region className="ml-auto flex shrink-0 items-center gap-1">
        <button
          title={volume === 0 ? "Unmute" : "Mute"}
          onClick={() => player.toggleMute()}
          className="flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <VolumeIcon className="size-4" />
        </button>
        <input
          type="range"
          title="Volume"
          min={0}
          max={1}
          step={0.01}
          value={volume}
          style={{ "--fill": `${volume * 100}%` } as React.CSSProperties}
          onChange={(e) => player.setVolume(Number(e.target.value))}
          className="w-20"
        />
      </div>
    </header>
  );
}
