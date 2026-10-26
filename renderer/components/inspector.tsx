import { ChevronDown, FolderOpen, Play, ScanSearch, Square, Star } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { CategoryLabel, Playhead } from "@/components/results";
import { WaveCanvas } from "@/components/wave-canvas";
import { api, drawnSeconds, isGuest, type Row } from "@/lib/api";
import { player, useFailed, usePlaying } from "@/lib/audio";
import { dragOut, pressOrDrag } from "@/lib/drag";
import { formatDuration, formatFormat, formatTempo } from "@/lib/format";
import { peekThumbnail, useDark, type WaveData } from "@/lib/waveform";

const DETAIL_COLUMNS = 2048;

function Chip({ children, title }: { children: React.ReactNode; title?: string }) {
  return (
    <span title={title} className="rounded-md border px-1.5 py-px text-xs">
      {children}
    </span>
  );
}

const source = (row: Row) =>
  row.chosen
    ? "Chosen by you"
    : row.categoryGuessed
      ? "Guessed from how it sounds"
      : "From its name";

/** What the sample is, and the menu to say otherwise. */
function Category({
  row,
  onChoose,
}: {
  row: Row;
  onChoose: (row: Row, x: number, y: number) => void;
}) {
  if (isGuest(row)) {
    return row.category ? (
      <Chip>
        <CategoryLabel row={row} />
      </Chip>
    ) : null;
  }
  return (
    <button
      title={row.category ? source(row) : undefined}
      onClick={(e) => {
        // The menu drops from under the chip.
        const box = e.currentTarget.getBoundingClientRect();
        onChoose(row, box.left, box.bottom + 4);
      }}
      className="flex items-center gap-0.5 rounded-md border py-px pr-1 pl-1.5 text-xs hover:bg-accent"
    >
      {row.category ? (
        <CategoryLabel row={row} />
      ) : (
        <span className="text-muted-foreground italic">no category</span>
      )}
      <ChevronDown className="size-3 text-muted-foreground" />
    </button>
  );
}

function Button({
  title,
  onClick,
  children,
}: {
  title: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className="flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-[13px] hover:bg-accent"
    >
      {children}
    </button>
  );
}

export function Inspector({
  row,
  onFindSimilar,
  onToggleFavorite,
  onChooseCategory,
}: {
  row: Row | null;
  onFindSimilar: (row: Row) => void;
  onToggleFavorite: (row: Row) => void;
  onChooseCategory: (row: Row, x: number, y: number) => void;
}) {
  const playing = usePlaying();
  const failed = useFailed();
  const dark = useDark();
  const [detail, setDetail] = useState<{ id: number; data: WaveData } | null>(null);
  const [hover, setHover] = useState<number | null>(null);
  const wave = useRef<HTMLDivElement>(null);
  const id = row?.id;

  useEffect(() => {
    if (id === undefined) return;
    let live = true;
    void api.waveform(id, DETAIL_COLUMNS).then((data) => {
      if (live) setDetail({ id, data });
    });
    return () => {
      live = false;
    };
  }, [id]);

  if (!row) {
    return (
      <section className="flex h-44 shrink-0 items-center justify-center border-t text-[13px] text-muted-foreground">
        Select a sample to hear it.
      </section>
    );
  }

  const data = detail?.id === row.id ? detail.data : peekThumbnail(row.id);
  const isPlaying = playing?.id === row.id;
  const fraction = (e: { clientX: number }) => {
    const box = wave.current!.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - box.left) / box.width));
  };
  const pitch = row.note ?? row.key;

  return (
    <section className="flex h-44 shrink-0 flex-col gap-2.5 border-t px-4 pt-3 pb-4">
      <div className="flex items-center gap-3">
        <button
          title={isPlaying ? "Stop (space)" : "Play (space)"}
          onClick={() => (isPlaying ? player.stop() : void player.play(row.id))}
          className="flex size-8 shrink-0 items-center justify-center rounded-full border hover:bg-accent"
        >
          {isPlaying ? (
            <Square className="size-3 fill-current" />
          ) : (
            <Play className="ml-0.5 size-3.5 fill-current" />
          )}
        </button>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h2 className="truncate text-[15px] font-medium">{row.name}</h2>
            {!isGuest(row) && (
              <button
                title={row.favorite ? "Remove from favorites (F)" : "Add to favorites (F)"}
                onClick={() => onToggleFavorite(row)}
                className="text-muted-foreground hover:text-foreground"
              >
                <Star className={`size-4 ${row.favorite ? "fill-current text-foreground" : ""}`} />
              </button>
            )}
          </div>
          <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className="truncate">{row.folder}</span>
          </div>
        </div>
        <Button title="Find sounds like this one (S)" onClick={() => onFindSimilar(row)}>
          <ScanSearch className="size-3.5" />
          find similar
        </Button>
        <Button title="Show in Finder" onClick={() => void api.reveal(row.path)}>
          <FolderOpen className="size-3.5" />
          reveal
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <Category row={row} onChoose={onChooseCategory} />
        <Chip>{row.looped ? "loop" : "one-shot"}</Chip>
        {pitch && <Chip title={row.note ? "Pitch" : "Key"}>{pitch}</Chip>}
        {row.tempo && <Chip>{formatTempo(row.tempo)} bpm</Chip>}
        <Chip>{formatDuration(row.duration)}</Chip>
        <span className="ml-1 text-xs text-muted-foreground">{formatFormat(row)}</span>
        {failed === row.id && (
          <span className="ml-auto text-xs text-muted-foreground">
            This file can't be played here.
          </span>
        )}
      </div>

      <div
        ref={wave}
        title="Click to play from here, drag to take the file along"
        onMouseDown={(e) => {
          const at = fraction(e);
          pressOrDrag(e, {
            onClick: () => void player.play(row.id, at * drawnSeconds(row.duration)),
            onDrag: () => dragOut(row, dark),
          });
        }}
        onMouseMove={(e) => setHover(fraction(e))}
        onMouseLeave={() => setHover(null)}
        className="relative min-h-0 flex-1 rounded-md"
      >
        <WaveCanvas data={data} className="absolute inset-0 size-full" />
        <Playhead id={row.id} />
        {hover !== null && (
          <>
            <div
              className="pointer-events-none absolute inset-y-0 w-px bg-foreground/25"
              style={{ left: `${hover * 100}%` }}
            />
            <div
              className="pointer-events-none absolute -top-0.5 rounded bg-background/80 px-1 text-[11px] text-muted-foreground tabular-nums"
              style={{ left: `calc(${hover * 100}% + 4px)` }}
            >
              {formatDuration(hover * drawnSeconds(row.duration))}
            </div>
          </>
        )}
      </div>
    </section>
  );
}
